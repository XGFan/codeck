import SwiftUI
import UIKit

/// Dims the Transcript behind a `RequestPanel`, which can still be scrolled through it.
struct RequestScrim: View {
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        Color.black
            .opacity(colorScheme == .dark ? 0.28 : 0.14)
            .ignoresSafeArea()
            .allowsHitTesting(false)
    }
}

/// What the Session waits on, in a panel rising from the bottom of the screen in place of the input box. The answers
/// scroll inside it when they do not fit under `maxHeight`.
struct RequestPanel: View {
    let requests: [ConnRequest]
    let c: ConnState
    var error: String?
    var onCloseError: () -> Void = {}
    let maxHeight: CGFloat
    @State private var contentHeight: CGFloat = 0

    /// About 70 % of the screen with the bottom safe area, and never under the header.
    static func heightLimit(screen: CGFloat, belowHeader: CGFloat) -> CGFloat {
        min(screen * 0.66, belowHeader)
    }

    private static let fill = Color(uiColor: UIColor { trait in
        trait.userInterfaceStyle == .dark ? UIColor(rgb: 0x303034, alpha: 0.96) : UIColor(rgb: 0xFFFFFF, alpha: 0.97)
    })
    private static let shape = UnevenRoundedRectangle(topLeadingRadius: 30, topTrailingRadius: 30, style: .continuous)

    var body: some View {
        VStack(spacing: 10) {
            if let error {
                ErrorLine(text: error, onGlass: false, onClose: onCloseError)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    ForEach(Array(requests.enumerated()), id: \.element.id) { i, request in
                        if i > 0 { Divider() }
                        RequestCard(request: request, c: c)
                    }
                }
                .onGeometryChange(for: CGFloat.self, of: \.size.height) { contentHeight = $0 }
            }
            // As tall as what it holds, up to what is left of `maxHeight` under the panel's own padding and error line.
            .frame(height: max(0, min(contentHeight, maxHeight - (error == nil ? 26 : 80))))
            .scrollBounceBehavior(.basedOnSize)
        }
        .padding(.horizontal, 16)
        .padding(.top, 18)
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity)
        .background {
            Self.shape
                .fill(Self.fill)
                .overlay(Self.shape.stroke(Palette.hairline, lineWidth: 0.5))
                .shadow(color: .black.opacity(0.14), radius: 15, y: -8)
                .ignoresSafeArea(edges: .bottom)
        }
    }
}
