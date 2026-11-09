import Foundation

/// Errors shown to the user as-is (`errorDescription`).
enum HostError: Error, Equatable, LocalizedError {
    case unreachable(String)
    case notCoDeck
    case versionMismatch(host: Int)
    case host(code: String?, message: String)
    case badResponse

    var errorDescription: String? {
        switch self {
        case .unreachable(let address): "连不上 Host（\(address)）。"
        case .notCoDeck: "该地址上运行的不是 CoDeck Host。"
        case .versionMismatch(let v):
            "Host 与 App 版本不一致（Host 接口版本 \(v)，App 支持 \(HostClient.apiVersion)）。"
        case .host(_, let message): message
        case .badResponse: "Host 返回的数据无法识别。"
        }
    }
}

/// HTTP side of the Host API: `/health` and `POST /v1/rpc`.
struct HostClient: Sendable {
    /// The `apiVersion` this app speaks.
    static let apiVersion = 1

    let address: HostAddress
    var session: URLSession = .shared

    // MARK: Health

    /// Checks that the address runs a compatible CoDeck Host.
    func checkHealth() async throws -> HostHealth {
        var request = URLRequest(url: address.baseURL.appending(path: "health"))
        request.timeoutInterval = 5
        let (data, response) = try await send(request)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw HostError.notCoDeck }
        return try Self.validateHealth(data)
    }

    static func validateHealth(_ data: Data) throws -> HostHealth {
        guard let health = try? JSONDecoder().decode(HostHealth.self, from: data), health.product == "CoDeck" else {
            throw HostError.notCoDeck
        }
        guard health.apiVersion == apiVersion else { throw HostError.versionMismatch(host: health.apiVersion) }
        return health
    }

    // MARK: RPC

    func listSessions(_ query: SessionQuery = .all) async throws -> [SessionSummary] {
        try await call("list_sessions", args: ["query": query])
    }

    func listProjects() async throws -> [ProjectSummary] {
        try await call("list_projects", args: [String: String]())
    }

    func getSession(_ sessionId: String) async throws -> SessionDetail {
        try await call("get_session", args: ["sessionId": sessionId])
    }

    /// `{"method", "args"}` → `{"result"}`, or non-2xx `{"error": {"code", "message"}}`.
    func call<Args: Encodable & Sendable, Result: Decodable>(
        _ method: String,
        args: Args,
        as _: Result.Type = Result.self,
        timeout: TimeInterval = 20
    ) async throws -> Result {
        var request = URLRequest(url: address.baseURL.appending(path: "v1/rpc"))
        request.httpMethod = "POST"
        request.timeoutInterval = timeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(RPCRequest(method: method, args: args))
        let (data, response) = try await send(request)
        return try Self.decodeRPC(data, status: (response as? HTTPURLResponse)?.statusCode ?? 0)
    }

    static func decodeRPC<Result: Decodable>(_ data: Data, status: Int) throws -> Result {
        let decoder = JSONDecoder()
        guard (200..<300).contains(status) else {
            if let body = try? decoder.decode(RPCError.self, from: data) {
                throw HostError.host(code: body.error.code, message: body.error.message)
            }
            throw HostError.badResponse
        }
        do {
            return try decoder.decode(RPCResult<Result>.self, from: data).result
        } catch {
            throw HostError.badResponse
        }
    }

    private func send(_ request: URLRequest) async throws -> (Data, URLResponse) {
        do {
            return try await session.data(for: request)
        } catch {
            throw HostError.unreachable(address.display)
        }
    }

    private struct RPCRequest<Args: Encodable>: Encodable {
        var method: String
        var args: Args
    }

    private struct RPCResult<T: Decodable>: Decodable { var result: T }

    private struct RPCError: Decodable {
        struct Body: Decodable {
            var code: String?
            var message: String
        }
        var error: Body
    }
}
