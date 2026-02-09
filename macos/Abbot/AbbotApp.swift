import SwiftUI

@main
struct AbbotApp: App {
    @StateObject private var daemonManager = DaemonManager()

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environmentObject(daemonManager)
        }
        .defaultSize(width: 1200, height: 800)
    }
}
