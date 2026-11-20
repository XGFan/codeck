import Foundation

/// Something the Session waits on, shown as a card above the input box. Decoded by `type`; a type this app
/// does not know, or a known one that does not decode, is `.other` (a card without actions).
enum ConnRequest: Decodable, Equatable, Identifiable, Sendable {
    case permission(PermissionRequest)
    case question(QuestionRequest)
    case choice(ChoiceRequest)
    /// `dialog` and `unrecognized`: a screen ccrpc cannot read, shown as its lines.
    case screen(ScreenRequest)
    /// The `/btw` panel: its question and answer.
    case btw(BtwRequest)
    /// A rewind stopped at its confirm page.
    case rewind(RewindRequest)
    case other(type: String, requestId: String)

    var id: String {
        switch self {
        case .permission(let r): r.requestId
        case .question(let r): r.requestId
        case .choice(let r): r.requestId
        case .screen(let r): r.requestId
        case .btw(let r): r.requestId
        case .rewind(let r): r.requestId
        case .other(_, let id): id
        }
    }

    /// Asked by a Subagent (listed after the main line's).
    var agentId: String? {
        switch self {
        case .permission(let r): r.agentId
        case .question(let r): r.agentId
        default: nil
        }
    }

    private enum Keys: String, CodingKey { case type, requestId = "request_id" }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        let type = c.lenient(String.self, .type) ?? ""
        let id = try c.decode(String.self, forKey: .requestId)
        switch type {
        case "permission_request":
            if let r = try? PermissionRequest(from: decoder) { self = .permission(r); return }
        case "question":
            if let r = try? QuestionRequest(from: decoder) { self = .question(r); return }
        case "choice":
            if let r = try? ChoiceRequest(from: decoder) { self = .choice(r); return }
        case "dialog", "unrecognized":
            if let r = try? ScreenRequest(from: decoder) { self = .screen(r); return }
        case "btw":
            if let r = try? BtwRequest(from: decoder) { self = .btw(r); return }
        case "rewind":
            if let r = try? RewindRequest(from: decoder) { self = .rewind(r); return }
        default:
            break
        }
        self = .other(type: type, requestId: id)
    }
}

struct PermissionRequest: Decodable, Equatable, Sendable {
    struct Option: Decodable, Equatable, Sendable {
        var label: String
        var meaning: String
    }

    var requestId: String
    /// The tool call it asks for.
    var toolUseId: String
    var toolName: String
    var input: JSONValue?
    var agentId: String?
    var options: [Option]?
    var layoutSource: String?

    enum CodingKeys: String, CodingKey {
        case requestId = "request_id", toolUseId = "tool_use_id", toolName = "tool_name", input, agentId = "agent_id", options
        case layoutSource = "layout_source"
    }

    init(
        requestId: String, toolUseId: String, toolName: String, input: JSONValue?, options: [Option]? = nil,
        layoutSource: String? = nil
    ) {
        self.requestId = requestId
        self.toolUseId = toolUseId
        self.toolName = toolName
        self.input = input
        self.options = options
        self.layoutSource = layoutSource
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        toolUseId = try c.decode(String.self, forKey: .toolUseId)
        toolName = c.lenient(String.self, .toolName) ?? "工具"
        input = c.lenient(JSONValue.self, .input)
        agentId = c.lenient(String.self, .agentId)
        options = c.lenient(Lossy<Option>.self, .options)?.items
        layoutSource = c.lenient(String.self, .layoutSource)
    }

    /// The 「总是允许」 option, when the dialog offers one.
    var allowAlways: Option? { options?.first { $0.meaning == "allow_always" } }
}

struct AskQuestion: Decodable, Equatable, Sendable {
    struct Option: Decodable, Equatable, Sendable {
        var label: String
        var description: String?
    }

    var question: String
    var header: String?
    var options: [Option]
    var multiSelect: Bool

    enum CodingKeys: String, CodingKey { case question, header, options, multiSelect }

    init(question: String, header: String? = nil, options: [Option] = [], multiSelect: Bool = false) {
        self.question = question
        self.header = header
        self.options = options
        self.multiSelect = multiSelect
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        question = c.lenient(String.self, .question) ?? ""
        header = c.lenient(String.self, .header)
        options = c.lenient(Lossy<Option>.self, .options)?.items ?? []
        multiSelect = c.lenient(Bool.self, .multiSelect) ?? false
    }
}

struct QuestionRequest: Decodable, Equatable, Sendable {
    var requestId: String
    /// The tool call it asks for.
    var toolUseId: String
    var questions: [AskQuestion]
    var agentId: String?

    enum CodingKeys: String, CodingKey {
        case requestId = "request_id", toolUseId = "tool_use_id", questions, agentId = "agent_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        toolUseId = try c.decode(String.self, forKey: .toolUseId)
        questions = c.lenient(Lossy<AskQuestion>.self, .questions)?.items ?? []
        agentId = c.lenient(String.self, .agentId)
    }
}

struct ChoiceRequest: Decodable, Equatable, Sendable {
    struct Option: Decodable, Equatable, Sendable {
        var label: String
        var meaning: String?
        var description: String?
    }

    var requestId: String
    var title: String
    var options: [Option]

    enum CodingKeys: String, CodingKey { case requestId = "request_id", title, options }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        title = c.lenient(String.self, .title) ?? ""
        options = c.lenient(Lossy<Option>.self, .options)?.items ?? []
    }
}

struct BtwRequest: Decodable, Equatable, Sendable {
    var requestId: String
    var question: String
    /// Nil until answered.
    var text: String?

    enum CodingKeys: String, CodingKey { case requestId = "request_id", question, text }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        question = c.lenient(String.self, .question) ?? ""
        text = c.lenient(String.self, .text)
    }
}

/// `rewind`'s answer, or a Fork's own rewind: the terminal stopped at the confirm page of going back to before a Main
/// Line prompt, answered with `answer_rewind`.
struct RewindRequest: Decodable, Equatable, Sendable {
    /// What a rewind restores, in the order the card lists them.
    enum Option: String, CaseIterable, Sendable {
        case both, conversation, code
    }

    var requestId: String
    /// The prompt's transcript uuid.
    var uuid: String
    /// The prompt's full text.
    var text: String
    /// What the page says on the code; nil for a sentence ccrpc could not read.
    var code: Code?
    /// The page's own sentence on the code.
    var codeText: String?
    /// The page's `⚠` warning when code would be restored.
    var warning: String?
    /// The restores the page offers.
    var options: [Option]

    enum CodingKeys: String, CodingKey {
        case requestId = "request_id", uuid, text, code, codeText = "code_text", warning, options
    }

    /// `code` of a rewind: whether the code is restored, with the diff and the files.
    struct Code: Decodable, Equatable, Sendable {
        var restores: Bool
        var delta: String?
        var target: String?

        init(restores: Bool, delta: String? = nil, target: String? = nil) {
            self.restores = restores
            self.delta = delta
            self.target = target
        }
    }

    private struct Offered: Decodable {
        var option: String
    }

    init(
        requestId: String, uuid: String, text: String = "", code: Code? = nil, codeText: String? = nil,
        warning: String? = nil, options: [Option]
    ) {
        self.requestId = requestId
        self.uuid = uuid
        self.text = text
        self.code = code
        self.codeText = codeText
        self.warning = warning
        self.options = options
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        uuid = try c.decode(String.self, forKey: .uuid)
        text = c.lenient(String.self, .text) ?? ""
        code = c.lenient(Code.self, .code)
        codeText = c.lenient(String.self, .codeText)
        warning = c.lenient(String.self, .warning)
        let offered = Set(c.lenient(Lossy<Offered>.self, .options)?.items.map(\.option) ?? [])
        options = Option.allCases.filter { offered.contains($0.rawValue) }
    }
}

struct ScreenRequest: Decodable, Equatable, Sendable {
    var requestId: String
    /// `dialog` or `unrecognized`.
    var type: String
    /// What the dialog waits for (`dialog`), or where ccrpc met the screen (`unrecognized`).
    var context: String?
    var lines: [String]

    enum CodingKeys: String, CodingKey {
        case requestId = "request_id", type, waitingFor = "waiting_for", context, lines
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        type = try c.decode(String.self, forKey: .type)
        context = c.lenient(String.self, .context) ?? c.lenient(String.self, .waitingFor)
        lines = c.lenient(Lossy<String>.self, .lines)?.items ?? []
    }
}
