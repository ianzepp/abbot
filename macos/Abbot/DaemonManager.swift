import Foundation

/// Manages the abbotd daemon lifecycle from the macOS app.
/// Spawns the daemon if not already running, monitors health, and
/// terminates on app quit (only if this app started it).
@MainActor
final class DaemonManager: ObservableObject {
    @Published var isRunning = false
    @Published var isStarting = false
    @Published var error: String?

    private var process: Process?
    private var healthTimer: Timer?
    private var weStartedDaemon = false

    private let healthURL = URL(string: "http://127.0.0.1:8080/health")!
    private let maxHealthRetries = 30
    private let healthRetryInterval: TimeInterval = 1.0

    /// Path to the bundled abbotd binary inside the .app bundle.
    var daemonPath: String {
        Bundle.main.resourceURL!.appendingPathComponent("abbotd").path
    }

    /// Path to the bundled web-dist directory inside the .app bundle.
    var webDistPath: String {
        Bundle.main.resourceURL!.appendingPathComponent("web-dist").path
    }

    /// Ensure the daemon is running. Checks health first — if an external
    /// daemon is already running, uses that. Otherwise spawns from the bundle.
    func ensureDaemonRunning() async {
        isStarting = true
        error = nil

        // Check if daemon is already running externally
        if await checkHealth() {
            isRunning = true
            isStarting = false
            weStartedDaemon = false
            return
        }

        // Spawn the daemon
        do {
            try spawnDaemon()
            weStartedDaemon = true

            // Poll health until ready
            let ready = await waitForHealth()
            if ready {
                isRunning = true
            } else {
                error = "Daemon started but did not become healthy within \(maxHealthRetries)s"
            }
        } catch {
            self.error = "Failed to start daemon: \(error.localizedDescription)"
        }

        isStarting = false
    }

    /// Stop the daemon if we started it.
    func stopIfOwned() {
        guard weStartedDaemon, let process = process, process.isRunning else { return }
        process.terminate()
        process.waitUntilExit()
        self.process = nil
        self.isRunning = false
        self.weStartedDaemon = false
    }

    // MARK: - Private

    private func spawnDaemon() throws {
        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: daemonPath)
        proc.arguments = ["run"]
        proc.environment = ProcessInfo.processInfo.environment.merging(
            ["ABBOT_WEB_DIST": webDistPath],
            uniquingKeysWith: { _, new in new }
        )

        // Send stdout/stderr to /dev/null — daemon logs to its own files
        proc.standardOutput = FileHandle.nullDevice
        proc.standardError = FileHandle.nullDevice

        proc.terminationHandler = { [weak self] proc in
            Task { @MainActor in
                guard let self = self else { return }
                if self.weStartedDaemon {
                    self.isRunning = false
                    if proc.terminationStatus != 0 {
                        self.error = "Daemon exited with status \(proc.terminationStatus)"
                    }
                }
            }
        }

        try proc.run()
        self.process = proc
    }

    private func checkHealth() async -> Bool {
        do {
            let (_, response) = try await URLSession.shared.data(from: healthURL)
            if let http = response as? HTTPURLResponse, http.statusCode == 200 {
                return true
            }
        } catch {
            // Not running
        }
        return false
    }

    private func waitForHealth() async -> Bool {
        for _ in 0..<maxHealthRetries {
            try? await Task.sleep(nanoseconds: UInt64(healthRetryInterval * 1_000_000_000))
            if await checkHealth() {
                return true
            }
        }
        return false
    }
}
