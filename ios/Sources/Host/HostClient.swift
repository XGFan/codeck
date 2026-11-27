import Foundation

/// Errors shown to the user as-is (`errorDescription`).
enum HostError: Error, Equatable, LocalizedError {
    case unreachable(String)
    case notCoDeck
    case versionMismatch(host: Int)
    case host(code: String?, message: String)
    case badResponse
    case removed
    case certificateChanged
    case pairingExpired
    case pairingUnreachable

    var errorDescription: String? {
        switch self {
        case .unreachable(let address): "连不上 Host（\(address)）。"
        case .notCoDeck: "该地址上运行的不是 CoDeck Host。"
        case .versionMismatch(let v):
            "Host 与 App 版本不一致（Host 接口版本 \(v)，App 支持 \(HostClient.apiVersion)）。"
        case .host(_, let message): message
        case .badResponse: "Host 返回的数据无法识别。"
        case .removed: "这台 iPhone 已被 Host 移除，请重新扫码配对。"
        case .certificateChanged: "Host 的证书与配对时不同，请重新扫码配对。"
        case .pairingExpired: "配对码已过期或已用过，请在 Host 上重新生成。"
        case .pairingUnreachable: "连不上二维码里的 Host。"
        }
    }
}

/// HTTP side of the Host API: `/health`, `/v1/pair`, `POST /v1/rpc`, and authenticated `GET`.
struct HostClient: Sendable {
    /// The `apiVersion` this app speaks.
    static let apiVersion = 2

    let address: HostAddress
    let fingerprint: String?
    let token: String?
    let session: URLSession
    let pinnedSession: PinnedSession?

    var baseURL: URL {
        URL(string: "\(fingerprint != nil ? "https" : "http")://\(address.host):\(address.port)")!
    }

    init(host: PairedHost, session: URLSession? = nil, pinnedSession: PinnedSession? = nil) {
        self.init(
            address: host.address,
            fingerprint: host.fingerprint,
            token: host.token,
            session: session,
            pinnedSession: pinnedSession
        )
    }

    init(
        address: HostAddress,
        fingerprint: String? = nil,
        token: String? = nil,
        session: URLSession? = nil,
        pinnedSession: PinnedSession? = nil
    ) {
        self.address = address
        self.fingerprint = fingerprint
        self.token = token
        if let session {
            self.session = session
            self.pinnedSession = pinnedSession
        } else if let pinnedSession {
            self.session = pinnedSession.session
            self.pinnedSession = pinnedSession
        } else {
            let owned = PinnedSession(expectedFingerprint: fingerprint)
            self.session = owned.session
            self.pinnedSession = owned
        }
    }

    // MARK: Health & Pairing

    /// Checks that the address runs a compatible CoDeck Host. Does not send Authorization.
    func checkHealth(timeout: TimeInterval = 5) async throws -> HostHealth {
        var request = URLRequest(url: baseURL.appending(path: "health"))
        request.timeoutInterval = timeout
        let (data, http) = try await send(request)
        guard http.statusCode == 200 else { throw HostError.notCoDeck }
        return try Self.validateHealth(data)
    }

    static func validateHealth(_ data: Data) throws -> HostHealth {
        guard let health = try? JSONDecoder().decode(HostHealth.self, from: data), health.product == "CoDeck" else {
            throw HostError.notCoDeck
        }
        guard health.apiVersion == apiVersion else { throw HostError.versionMismatch(host: health.apiVersion) }
        return health
    }

    /// Exchanges a one-time pairing key for a device id and bearer token. Does not send Authorization.
    func pair(key: String, name: String, timeout: TimeInterval = 10) async throws -> (deviceId: String, token: String) {
        var request = URLRequest(url: baseURL.appending(path: "v1/pair"))
        request.httpMethod = "POST"
        request.timeoutInterval = timeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(PairRequest(key: key, name: name))
        let (data, http) = try await send(request)
        return try Self.decodePair(data, status: http.statusCode)
    }

    static func decodePair(_ data: Data, status: Int) throws -> (deviceId: String, token: String) {
        if status == 401 { throw HostError.removed }
        let decoder = JSONDecoder()
        if let body = try? decoder.decode(RPCError.self, from: data) {
            if status == 403 || body.error.code == "pairing_expired" {
                throw HostError.pairingExpired
            }
            throw HostError.host(code: body.error.code, message: body.error.message)
        }
        if status == 403 { throw HostError.pairingExpired }
        guard (200..<300).contains(status) else { throw HostError.badResponse }
        do {
            let payload = try decoder.decode(RPCResult<PairPayload>.self, from: data).result
            guard !payload.deviceId.isEmpty, !payload.token.isEmpty else { throw HostError.badResponse }
            return (payload.deviceId, payload.token)
        } catch let error as HostError {
            throw error
        } catch {
            throw HostError.badResponse
        }
    }

    /// Authenticated probe (`get_app_info`) used to verify an existing token or attribute disconnects.
    func checkAuth(timeout: TimeInterval = 5) async throws {
        let _: JSONValue = try await call("get_app_info", args: [String: String](), as: JSONValue.self, timeout: timeout)
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

    /// The Projects in the desktop's sidebar; nil before any were added.
    func sidebarProjects() async throws -> [SidebarProject]? {
        try await call("get_sidebar_projects", args: [String: String]())
    }

    /// Directories on the Host completing a typed path (each ends in `/`).
    func completeDirectories(_ partial: String) async throws -> [String] {
        try await call("complete_paths", args: CompletePaths(partial: partial))
    }

    private struct CompletePaths: Encodable, Sendable {
        var partial: String
        enum CodingKeys: String, CodingKey { case cwd, partial, files }
        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encodeNil(forKey: .cwd)
            try c.encode(partial, forKey: .partial)
            try c.encode(false, forKey: .files)
        }
    }

    /// `{"method", "args"}` → `{"result"}` or `{"error": {"code", "message"}}`.
    func call<Args: Encodable & Sendable, Result: Decodable>(
        _ method: String,
        args: Args,
        as _: Result.Type = Result.self,
        timeout: TimeInterval = 20
    ) async throws -> Result {
        var request = URLRequest(url: baseURL.appending(path: "v1/rpc"))
        request.httpMethod = "POST"
        request.timeoutInterval = timeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&request)
        request.httpBody = try JSONEncoder().encode(RPCRequest(method: method, args: args))
        let (data, http) = try await send(request)
        return try Self.decodeRPC(data, status: http.statusCode)
    }

    /// Forwards an RPC for the Transcript web bridge, returning the Host's raw JSON response body
    /// without decoding `result`.
    func forwardRPC(method: String, args: JSONValue, timeout: TimeInterval = 20) async throws -> Data {
        var request = URLRequest(url: baseURL.appending(path: "v1/rpc"))
        request.httpMethod = "POST"
        request.timeoutInterval = timeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        authorize(&request)
        request.httpBody = try JSONEncoder().encode(RPCRequest(method: method, args: args))
        let (data, http) = try await send(request)
        if (200..<300).contains(http.statusCode) {
            return data
        }
        if (try? JSONDecoder().decode(RPCError.self, from: data)) != nil {
            return data
        }
        throw HostError.badResponse
    }

    /// Authenticated `GET` against the Host (used by the Transcript scheme handler).
    func get(path: String, query: String? = nil, timeout: TimeInterval = 20) async throws -> (Data, HTTPURLResponse) {
        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)!
        components.path = path.hasPrefix("/") ? path : "/" + path
        if let query, !query.isEmpty {
            // Preserve existing percent-encoding when valid; fall back to encoding raw strings.
            if query.unicodeScalars.allSatisfy({ Self.encodedQueryAllowed.contains($0) }) {
                components.percentEncodedQuery = query
            } else {
                components.query = query
            }
        }
        guard let url = components.url else { throw HostError.badResponse }
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.timeoutInterval = timeout
        authorize(&request)
        return try await send(request)
    }

    private static let encodedQueryAllowed: CharacterSet = {
        var set = CharacterSet.urlQueryAllowed
        set.insert(charactersIn: "%")
        return set
    }()

    /// `{"result"}`, or `{"error": {code, message}}`, which the Host sends with HTTP 200 (and any other status).
    /// HTTP 401 always maps to `.removed` regardless of body.
    static func decodeRPC<Result: Decodable>(_ data: Data, status: Int) throws -> Result {
        if status == 401 { throw HostError.removed }
        let decoder = JSONDecoder()
        if let body = try? decoder.decode(RPCError.self, from: data) {
            throw HostError.host(code: body.error.code, message: body.error.message)
        }
        guard (200..<300).contains(status) else { throw HostError.badResponse }
        do {
            return try decoder.decode(RPCResult<Result>.self, from: data).result
        } catch {
            throw HostError.badResponse
        }
    }

    private func authorize(_ request: inout URLRequest) {
        if let token, !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
    }

    private func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response): (Data, URLResponse)
        do {
            if let pinnedSession {
                (data, response) = try await pinnedSession.data(for: request)
            } else {
                (data, response) = try await session.data(for: request)
            }
        } catch let error as HostError {
            throw error
        } catch {
            throw HostError.unreachable(address.display)
        }
        guard let http = response as? HTTPURLResponse else { throw HostError.badResponse }
        if http.statusCode == 401 { throw HostError.removed }
        return (data, http)
    }

    private struct PairRequest: Encodable {
        var key: String
        var name: String
    }

    private struct PairPayload: Decodable {
        var deviceId: String
        var token: String
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
