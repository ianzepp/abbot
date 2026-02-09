import SwiftUI

/// Routes between the first-launch setup view and the main web content view.
struct ContentView: View {
    @EnvironmentObject var daemonManager: DaemonManager
    @State private var isSetupComplete = ConfigWriter.configExists

    var body: some View {
        Group {
            if !isSetupComplete {
                SetupView(isSetupComplete: $isSetupComplete)
            } else if daemonManager.isStarting {
                startingView
            } else if let error = daemonManager.error {
                errorView(error)
            } else if daemonManager.isRunning {
                WebContentView()
            } else {
                startingView
                    .task {
                        await daemonManager.ensureDaemonRunning()
                    }
            }
        }
        .onChange(of: isSetupComplete) { complete in
            if complete {
                Task {
                    await daemonManager.ensureDaemonRunning()
                }
            }
        }
        .onDisappear {
            daemonManager.stopIfOwned()
        }
    }

    private var startingView: some View {
        VStack(spacing: 16) {
            ProgressView()
                .controlSize(.large)
            Text("Starting Abbot daemon...")
                .font(.headline)
                .foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func errorView(_ message: String) -> some View {
        VStack(spacing: 16) {
            Text("Failed to Start")
                .font(.headline)
            Text(message)
                .foregroundColor(.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 40)
            Button("Retry") {
                Task {
                    await daemonManager.ensureDaemonRunning()
                }
            }
            .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
