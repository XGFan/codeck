import PhotosUI
import SwiftUI

/// The button at the end of the input box.
enum ComposerAction: Equatable {
    case stop, steer, send

    static func of(state: ConnState.Phase?, isBusy: Bool, hasContent: Bool, uploading: Bool) -> ComposerAction {
        if isBusy, !hasContent, !uploading { return .stop }
        return state == .busy ? .steer : .send
    }
}

/// The input box (Composer.tsx): idle → `prompt`, or `slash` for a built-in command; busy → `steer` (插话, delivered
/// when the current tool call ends); waiting → nothing (the card comes first). Photos are saved on the Host first and sent as `parts`, the
/// text before the images. With the box empty while busy, the button stops the turn (`abort`).
struct Composer: View {
    let c: ConnState
    let sessionId: String
    @Binding var text: String
    @Binding var images: [ComposerImage]
    var focused: FocusState<Bool>.Binding
    @Environment(HostModel.self) private var model
    @State private var pickerItems: [PhotosPickerItem] = []
    @State private var sending = false
    @State private var sent = 0

    private var body_: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var uploading: Bool { images.contains { $0.path == nil && !$0.failed } }
    private var hasContent: Bool { !body_.isEmpty || images.contains { $0.path != nil } }
    private var live: Bool { c.canCommand }

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            VStack(alignment: .leading, spacing: 0) {
                if !images.isEmpty {
                    thumbnails
                        .padding(.horizontal, 12)
                        .padding(.top, 8)
                }
                HStack(alignment: .bottom, spacing: 2) {
                    PhotosPicker(selection: $pickerItems, maxSelectionCount: 6, matching: .images) {
                        Image(systemName: "photo")
                            .font(.system(size: 20))
                            .foregroundStyle(Palette.accentText)
                            .frame(width: 44, height: 44)
                    }
                    .disabled(!live)
                    .accessibilityLabel("添加图片")
                    .padding(.vertical, 4)
                    TextField(placeholder, text: $text, axis: .vertical)
                        .font(.system(size: 16))
                        .lineLimit(1...6)
                        .focused(focused)
                        .disabled(!live)
                        .padding(.vertical, 15)
                }
                .padding(.leading, 4)
                .padding(.trailing, 16)
            }
            .glass(in: RoundedRectangle(cornerRadius: 26, style: .continuous))
            .opacity(live && c.state != .waiting ? 1 : 0.6)
            sendButton
        }
        .sensoryFeedback(.success, trigger: sent)
        .onChange(of: pickerItems) { _, items in
            guard !items.isEmpty else { return }
            pickerItems = []
            for item in items { add(item) }
        }
    }

    private var placeholder: String {
        guard live else { return "启动中…" }
        switch c.state {
        case .busy: return "插话，当前工具调用结束后送达"
        case .waiting: return "先回答上面的请求"
        case .idle: return "给 Claude 发消息"
        default: return "启动中…"
        }
    }

    // MARK: Send / stop

    @ViewBuilder
    private var sendButton: some View {
        switch ComposerAction.of(state: c.state, isBusy: c.isBusy, hasContent: hasContent, uploading: uploading) {
        case .stop:
            Button { Task { await abort() } } label: {
                RoundedRectangle(cornerRadius: 3)
                    .fill(.white)
                    .frame(width: 14, height: 14)
                    .frame(width: 52, height: 52)
                    .background(Palette.accent, in: Circle())
                    .shadow(color: .black.opacity(0.12), radius: 9, y: 6)
            }
            .accessibilityLabel("中止")
            .buttonStyle(.plain)
            .disabled(!live)
            .sensoryFeedback(.impact, trigger: sending)
        case .steer:
            // 插话: delivered when the current tool call ends.
            Button { Task { await send("steer") } } label: {
                Text("插话")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 20)
                    .frame(height: 52)
                    .background(canSend ? Palette.accent : Self.offFill, in: Capsule())
            }
            .buttonStyle(.plain)
            .disabled(!canSend)
        case .send:
            let enabled = canSend && c.state == .idle
            Button { Task { await send("prompt") } } label: {
                Image(systemName: "arrow.up")
                    .font(.system(size: 20, weight: .bold))
                    .foregroundStyle(.white)
                    .frame(width: 52, height: 52)
                    .background(enabled ? Palette.accent : Self.offFill, in: Circle())
            }
            .accessibilityLabel("发送")
            .buttonStyle(.plain)
            .disabled(!enabled)
        }
    }

    private var canSend: Bool { live && hasContent && !sending && !uploading }

    private static let offFill = Color(uiColor: UIColor { trait in
        trait.userInterfaceStyle == .dark ? UIColor(rgb: 0xFFFFFF, alpha: 0.18) : UIColor(rgb: 0x787880, alpha: 0.32)
    })

    private func send(_ type: String) async {
        guard canSend else { return }
        let body = body_
        let paths = images.compactMap(\.path)
        if let refused = SlashRules.refusedHere(body) { return model.connect.setError(sessionId, refused) }
        var list: [SlashCommand]?
        if body.hasPrefix("/") {
            sending = true
            list = await model.connect.slashCommands(cwd: c.cwd)
            sending = false
        }
        if type == "steer", let refused = SlashRules.refusedWhileBusy(body, list) {
            return model.connect.setError(sessionId, refused)
        }
        var cmd: [String: JSONValue] = ["type": .string(type)]
        if type == "prompt", let command = SlashRules.slashCommand(body, withImages: !paths.isEmpty, list) {
            cmd = ["type": "slash", "command": .string(command)]
        } else if paths.isEmpty {
            cmd["text"] = .string(body)
        } else {
            let text: [JSONValue] = body.isEmpty ? [] : [["text": .string(body)]]
            cmd["parts"] = .array(text + paths.map { ["image": .string($0)] })
        }
        let (keptText, keptImages) = (text, images)
        sending = true
        text = ""
        images = []
        let r = await model.connect.command(c.conn, .object(cmd))
        sending = false
        if r.success {
            sent += 1
            model.connect.setError(sessionId, nil)
        } else {
            // Back in the box, unless something new was typed meanwhile.
            if text.isEmpty, images.isEmpty { (text, images) = (keptText, keptImages) }
            model.connect.setError(sessionId, "发送失败：\(r.errorText)")
        }
    }

    private func abort() async {
        sending = true
        let r = await model.connect.command(c.conn, ["type": "abort"])
        sending = false
        if !r.success { model.connect.setError(sessionId, "中止失败：\(r.errorText)") }
    }

    // MARK: Images

    private var thumbnails: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(images) { image in
                    Image(uiImage: image.thumb)
                        .resizable()
                        .scaledToFill()
                        .frame(width: 56, height: 56)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .overlay {
                            if image.path == nil && !image.failed { ProgressView() }
                            if image.failed {
                                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.red)
                            }
                        }
                        .overlay(alignment: .topTrailing) {
                            Button("移除", systemImage: "xmark.circle.fill") {
                                images.removeAll { $0.id == image.id }
                            }
                            .labelStyle(.iconOnly)
                            .symbolRenderingMode(.palette)
                            .foregroundStyle(.white, .black.opacity(0.6))
                            .offset(x: 6, y: -6)
                        }
                }
            }
            .padding(.top, 6)
            .padding(.trailing, 6)
        }
    }

    private func add(_ item: PhotosPickerItem) {
        let id = UUID()
        Task {
            guard let data = try? await item.loadTransferable(type: Data.self),
                  let jpeg = await Task.detached(operation: { ComposerImage.jpeg(data) }).value,
                  let thumb = UIImage(data: jpeg)
            else {
                model.connect.setError(sessionId, "图片保存失败：无法读取这张图片")
                return
            }
            images.append(ComposerImage(id: id, thumb: thumb))
            do {
                let path = try await model.connect.saveImage(jpeg: jpeg)
                if let i = images.firstIndex(where: { $0.id == id }) { images[i].path = path }
            } catch {
                if let i = images.firstIndex(where: { $0.id == id }) { images[i].failed = true }
                model.connect.setError(sessionId, "图片保存失败：\(ConnectStore.text(error))")
            }
        }
    }
}

/// A photo in the box: saved on the Host once `path` is set.
struct ComposerImage: Identifiable {
    let id: UUID
    let thumb: UIImage
    var path: String?
    var failed = false

    /// JPEG at most 2048 px on its long side (a photo of 48 MP would be tens of MB as base64).
    nonisolated static func jpeg(_ data: Data, maxSide: CGFloat = 2048) -> Data? {
        guard let image = UIImage(data: data) else { return nil }
        let size = image.size
        let scale = min(1, maxSide / max(size.width, size.height, 1))
        let target = CGSize(width: (size.width * scale).rounded(), height: (size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let out = UIGraphicsImageRenderer(size: target, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: target))
        }
        return out.jpegData(compressionQuality: 0.85)
    }
}
