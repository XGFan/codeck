import SwiftUI
import UIKit

/// The line under the title of a Session page.
struct HeaderStatus: Equatable {
    var text: String
    var dot: Color
    /// How long it has been at it: stays readable when `text` is cut.
    var elapsed: String?

    /// A live Connect: its state and, while it works, what it is doing and for how long.
    static func connected(_ c: ConnState) -> HeaderStatus {
        var parts = [ConnectWords.state(c.state)]
        if c.state == .busy {
            if let doing = c.progress?.text ?? c.progress?.verb { parts.append(doing) }
        }
        let dot: Color = switch c.state {
        case .busy: Palette.ok
        case .waiting: Palette.warn
        default: Palette.idle
        }
        let elapsed = c.state == .busy ? c.progress?.elapsedS.map(ConnectWords.elapsed) : nil
        return HeaderStatus(text: parts.joined(separator: " · "), dot: dot, elapsed: elapsed)
    }
}

/// The top of a Session page, floating over its Transcript: back, the title with its status, and `trailing`.
struct FloatingHeader<Trailing: View>: View {
    let title: String
    var status: HeaderStatus?
    @ViewBuilder let trailing: Trailing
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        HStack(spacing: 12) {
            Button {
                dismiss()
            } label: {
                Image(systemName: "chevron.left")
            }
            .buttonStyle(GlassCircleButtonStyle())
            .accessibilityLabel("返回 Session 列表")
            VStack(spacing: 1) {
                Text(title).font(.system(size: 14, weight: .semibold)).lineLimit(1)
                if let status {
                    HStack(spacing: 5) {
                        Circle().fill(status.dot).frame(width: 6, height: 6)
                        Text(status.text).lineLimit(1)
                        if let elapsed = status.elapsed {
                            Text("· \(elapsed)").monospacedDigit().fixedSize()
                        }
                    }
                    .font(.system(size: 11.5))
                    .foregroundStyle(.secondary)
                }
            }
            .padding(.horizontal, 18)
            .frame(maxWidth: .infinity)
            .frame(height: 48)
            .glassCapsule()
            trailing.frame(width: 48, height: 48)
        }
        .padding(.horizontal, 16)
        .padding(.top, 1)
        .background { InteractivePopEnabler().frame(width: 0, height: 0) }
    }
}

/// Keeps the swipe from the left edge working with the navigation bar hidden: UIKit's own delegate refuses the
/// gesture then, so this one takes over while the page is up and hands it back when the page goes.
private struct InteractivePopEnabler: UIViewControllerRepresentable {
    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIViewController(context: Context) -> AppearingController {
        let vc = AppearingController()
        vc.view.isUserInteractionEnabled = false
        vc.onAppear = { [coordinator = context.coordinator] nav in coordinator.attach(to: nav) }
        return vc
    }

    func updateUIViewController(_: AppearingController, context _: Context) {}

    /// Its navigation controller is only certain to be there once it has appeared.
    final class AppearingController: UIViewController {
        var onAppear: (UINavigationController) -> Void = { _ in }

        override func viewDidAppear(_ animated: Bool) {
            super.viewDidAppear(animated)
            if let navigationController { onAppear(navigationController) }
        }
    }

    static func dismantleUIViewController(_: AppearingController, coordinator: Coordinator) {
        coordinator.detach()
    }

    @MainActor
    final class Coordinator {
        private var taken: PopTakeover?

        func attach(to nav: UINavigationController) {
            guard taken == nil else { return }
            taken = PopTakeover.acquire(nav)
        }

        func detach() {
            taken?.release()
            taken = nil
        }
    }
}

/// One per navigation stack, however many Session pages are on it (a Fork opens over its Origin), so pages leaving in
/// any order hand UIKit's delegate back exactly once, when the last one goes.
@MainActor
private final class PopTakeover: NSObject, UIGestureRecognizerDelegate {
    private static var current: PopTakeover?

    private weak var nav: UINavigationController?
    /// Strong: UIKit only holds a gesture's delegate weakly, and losing the original would leave the app with no
    /// edge swipe at all afterwards.
    private var original: (any UIGestureRecognizerDelegate)?
    private var holders = 0

    static func acquire(_ nav: UINavigationController) -> PopTakeover? {
        guard let pop = nav.interactivePopGestureRecognizer else { return nil }
        let takeover: PopTakeover
        if let current, current.nav === nav {
            takeover = current
        } else {
            takeover = PopTakeover()
            takeover.nav = nav
            takeover.original = pop.delegate
            pop.delegate = takeover
            pop.isEnabled = true
            current = takeover
        }
        takeover.holders += 1
        return takeover
    }

    func release() {
        holders -= 1
        guard holders == 0 else { return }
        if let pop = nav?.interactivePopGestureRecognizer, let original { pop.delegate = original }
        original = nil
        if Self.current === self { Self.current = nil }
    }

    func gestureRecognizerShouldBegin(_: UIGestureRecognizer) -> Bool {
        // A second swipe during a transition leaves the navigation stack in an undefined state.
        guard nav?.transitionCoordinator == nil else { return false }
        return (nav?.viewControllers.count ?? 0) > 1
    }

    /// The edge swipe goes first: it fails at once when it does not start at the edge, so nothing else waits on it in
    /// practice, while a slightly diagonal swipe is not taken by the Transcript's scrollers.
    func gestureRecognizer(
        _: UIGestureRecognizer, shouldBeRequiredToFailBy _: UIGestureRecognizer
    ) -> Bool { true }

    func gestureRecognizer(
        _: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith _: UIGestureRecognizer
    ) -> Bool { false }
}
