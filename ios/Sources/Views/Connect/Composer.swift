import PhotosUI
import SwiftUI

/// The input box (Composer.tsx): idle → `prompt`; busy → `steer` (插话, delivered when the current tool call
/// ends); 「排队（空闲后发送）」 → `follow_up`. Photos are saved on the Host first and sent as `parts`, the text
/// before the images. With the box empty while busy, the button stops the turn (`abort`).
struct Composer: View {
    let c: ConnState
    let sessionId: String
    @Environment(HostModel.self) private var model
    @State private var text = ""
    @State private var images: [ComposerImage] = []
    @State private var pickerItems: [PhotosPickerItem] = []
    @State private var sending = false
    @State private var sent = 0
    @FocusState private var focused: Bool

    private var body_: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var uploading: Bool { images.contains { $0.path == nil && !$0.failed } }
    private var hasContent: Bool { !body_.isEmpty || images.contains { $0.path != nil } }
    private var live: Bool { c.canCommand }

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            PhotosPicker(selection: $pickerItems, maxSelectionCount: 6, matching: .images) {
                Image(systemName: "photo.on.rectangle")
                    .font(.title3)
                    .frame(width: 36, height: 36)
            }
            .disabled(!live)
            .accessibilityLabel("添加图片")
            VStack(alignment: .leading, spacing: 8) {
                if !images.isEmpty { thumbnails }
                TextField(placeholder, text: $text, axis: .vertical)
                    .lineLimit(1...6)
                    .focused($focused)
                    .disabled(!live)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 18))
            sendButton
        }
        .opacity(live ? 1 : 0.6)
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
        if c.isBusy, !hasContent, !uploading {
            Button("中止", systemImage: "stop.circle.fill") { Task { await abort() } }
                .labelStyle(.iconOnly)
                .font(.system(size: 32))
                .disabled(!live)
                .sensoryFeedback(.impact, trigger: sending)
        } else if c.state == .busy {
            // Tap: 插话; hold: 排队 until idle.
            Menu {
                queueButton
            } label: {
                sendLabel(steer: true)
            } primaryAction: {
                Task { await send("steer") }
            }
            .disabled(!canSend)
        } else if c.state == .waiting {
            // A card comes first: only 排队 until idle.
            Menu { queueButton } label: { sendLabel(steer: false) }
                .disabled(!canSend)
        } else {
            Button { Task { await send("prompt") } } label: { sendLabel(steer: false) }
                .disabled(!canSend || c.state != .idle)
        }
    }

    private var queueButton: some View {
        Button("排队（空闲后发送）", systemImage: "clock") { Task { await send("follow_up") } }
    }

    private var canSend: Bool { live && hasContent && !sending && !uploading }

    @ViewBuilder
    private func sendLabel(steer: Bool) -> some View {
        if steer {
            Label("插话", systemImage: "arrow.up")
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 12)
                .frame(height: 32)
                .foregroundStyle(.white)
                .background(canSend ? AnyShapeStyle(.tint) : AnyShapeStyle(.gray.opacity(0.4)), in: Capsule())
        } else {
            Image(systemName: "arrow.up.circle.fill")
                .font(.system(size: 32))
                .accessibilityLabel("发送")
        }
    }

    private func send(_ type: String) async {
        guard canSend else { return }
        let body = body_
        let paths = images.compactMap(\.path)
        var cmd: [String: JSONValue] = ["type": .string(type)]
        if paths.isEmpty {
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
