import Foundation

/// Offers to symlink the bundled abbot/abbotd binaries to /usr/local/bin/
/// so users can access them from the terminal.
struct CLIInstaller {
    static let installDir = "/usr/local/bin"

    static let binaries = ["abbot", "abbotd"]

    /// Check if CLI tools are already installed (symlinked or otherwise).
    static var isInstalled: Bool {
        binaries.allSatisfy { name in
            FileManager.default.fileExists(atPath: "\(installDir)/\(name)")
        }
    }

    /// Install symlinks from /usr/local/bin/ → bundled binaries.
    /// Requires user authorization via AppleScript since /usr/local/bin/ may need elevated permissions.
    static func install() throws {
        let resourceURL = Bundle.main.resourceURL!

        // Ensure /usr/local/bin exists
        if !FileManager.default.fileExists(atPath: installDir) {
            try runPrivileged("mkdir -p \(installDir)")
        }

        for name in binaries {
            let source = resourceURL.appendingPathComponent(name).path
            let dest = "\(installDir)/\(name)"

            // Remove existing symlink/file if present
            if FileManager.default.fileExists(atPath: dest) {
                try runPrivileged("rm -f \(dest)")
            }

            try runPrivileged("ln -s '\(source)' '\(dest)'")
        }
    }

    /// Run a command with admin privileges via AppleScript.
    private static func runPrivileged(_ command: String) throws {
        let script = "do shell script \"\(command)\" with administrator privileges"
        var error: NSDictionary?
        if let appleScript = NSAppleScript(source: script) {
            appleScript.executeAndReturnError(&error)
            if let error = error {
                throw NSError(
                    domain: "CLIInstaller",
                    code: -1,
                    userInfo: [NSLocalizedDescriptionKey: error[NSAppleScript.errorMessage] ?? "Unknown error"]
                )
            }
        }
    }
}
