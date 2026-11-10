import SwiftUI
import UIKit

/// The app's colors, light and dark (the desktop's accent and Live colors, adjusted for fills on a phone).
enum Palette {
    /// Fills under white text.
    static let accent = Color(light: 0x0A5FCC, dark: 0x2F6FE0)
    /// The accent as text and icons, and the app's tint.
    static let accentText = Color(light: 0x0A5FCC, dark: 0x7FB0FF)
    /// A pale accent fill behind `accentText`.
    static let tint = Color(uiColor: UIColor { trait in
        trait.userInterfaceStyle == .dark
            ? UIColor(rgb: 0x4C8DF6, alpha: 0.18)
            : UIColor(rgb: 0x0A5FCC, alpha: 0.10)
    })
    static let ok = Color(light: 0x1E7F45, dark: 0x30D158)
    static let warn = Color(light: 0xE08A00, dark: 0xFFB340)
    static let warnText = Color(light: 0xA35F00, dark: 0xFFB340)
    static let idle = Color(light: 0x8E8E93, dark: 0x98989D)
    static let error = Color(light: 0xC4281C, dark: 0xFF8A80)
    /// Text fields and code on a card or panel.
    static let field = Color(uiColor: .systemGray6)
    /// A neutral button fill.
    static let neutral = Color(uiColor: .tertiarySystemFill)
    static let hairline = Color.primary.opacity(0.08)
    /// The Transcript page's background (`--color-ground` in `src/index.css`), for what must blend into it.
    static let transcriptGround = Color(light: 0xFFFFFF, dark: 0x1E1E20)
}

extension Color {
    init(light: UInt32, dark: UInt32) {
        self.init(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(rgb: dark) : UIColor(rgb: light) })
    }
}

extension UIColor {
    convenience init(rgb: UInt32, alpha: CGFloat = 1) {
        self.init(
            red: CGFloat((rgb >> 16) & 0xFF) / 255,
            green: CGFloat((rgb >> 8) & 0xFF) / 255,
            blue: CGFloat(rgb & 0xFF) / 255,
            alpha: alpha
        )
    }
}

extension View {
    /// The floating glass of the Session page: Liquid Glass where there is one, a material otherwise.
    @ViewBuilder
    func glass<S: InsettableShape>(in shape: S) -> some View {
        if #available(iOS 26, *) {
            glassEffect(.regular, in: shape)
        } else {
            background(.regularMaterial, in: shape)
                .overlay(shape.strokeBorder(Palette.hairline, lineWidth: 0.5))
                .shadow(color: .black.opacity(0.12), radius: 9, y: 6)
        }
    }

    func glassCapsule() -> some View { glass(in: Capsule()) }
}

/// A round floating button (back, 「···」).
struct GlassCircleButtonStyle: ButtonStyle {
    var size: CGFloat = 48

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 18, weight: .semibold))
            .foregroundStyle(.primary)
            .frame(width: size, height: size)
            .glass(in: Circle())
            .contentShape(Circle())
            .opacity(configuration.isPressed ? 0.6 : 1)
    }
}

/// The label of a control on the Session page's row of pills: 44 high; dimmed when it cannot be used now.
struct GlassPillLabel<Content: View>: View {
    var enabled = true
    @ViewBuilder let content: Content

    var body: some View {
        content
            .font(.footnote)
            .lineLimit(1)
            .foregroundStyle(enabled ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
            .padding(.horizontal, 14)
            .frame(height: 44)
            .glassCapsule()
            .opacity(enabled ? 1 : 0.7)
    }
}

/// A full-width capsule answer or action.
struct PillButtonStyle: ButtonStyle {
    enum Kind { case primary, tinted, neutral }

    var kind = Kind.primary
    var height: CGFloat = 52
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(kind == .primary ? .body.weight(.semibold) : .body)
            .frame(maxWidth: .infinity, minHeight: height)
            .padding(.horizontal, 12)
            .foregroundStyle(foreground)
            .background(background, in: Capsule())
            .contentShape(Capsule())
            .opacity(configuration.isPressed ? 0.6 : isEnabled ? 1 : 0.5)
    }

    private var foreground: Color {
        switch kind {
        case .primary: .white
        case .tinted: Palette.accentText
        case .neutral: .primary
        }
    }

    private var background: Color {
        switch kind {
        case .primary: Palette.accent
        case .tinted: Palette.tint
        case .neutral: Palette.neutral
        }
    }
}
