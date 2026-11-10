import SwiftUI

@main
struct CoDeckApp: App {
    @State private var model = HostModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            Group {
                if model.address != nil {
                    SessionListView()
                } else {
                    NavigationStack {
                        HostSetupView(current: nil) { model.connect(to: $0) }
                    }
                }
            }
            .environment(model)
            .tint(Palette.accentText)
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active: model.resume()
            case .background: model.pause()
            default: break
            }
        }
    }
}
