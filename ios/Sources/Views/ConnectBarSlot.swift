import SwiftUI

/// Seam for the native Connect bar (input box, approval / question cards) pinned to the bottom of
/// the Session page. It is empty until Connect lands; `SessionDetailView` already reserves it with
/// `safeAreaInset(edge: .bottom)`, so the Transcript scrolls above whatever this shows.
struct ConnectBarSlot: View {
    let session: SessionSummary?

    var body: some View {
        EmptyView()
    }
}
