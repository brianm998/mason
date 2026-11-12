// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// What clicking Open session does, apart from the window: ask the daemon about the task's
/// session, ask the owner if the step is running, build the `mason task open` line, and say so
/// when the terminal could not be started. The actual terminal spawn is the app's (`launcher`).
public enum OpenSession {
    public enum Decision: Equatable, Sendable {
        /// The step is running: ask before a second session collides with it.
        case confirm
        /// Start the terminal with this command.
        case launch(anyway: Bool)
        /// The daemon said no; show its words.
        case failed(String)
    }

    /// Where the installer puts the CLI. Never found through PATH, which a new shell may lack.
    public static func defaultCLI(home: String = NSHomeDirectory()) -> String {
        home + "/.config/mason/bin/mason"
    }

    /// The daemon's reply to `GET /tasks/{id}/session`, or its error.
    public static func decide(ok: Bool, json: String, error: String) -> Decision {
        guard ok else { return .failed(error.isEmpty ? "The daemon could not open that task's session." : error) }
        guard let d = json.data(using: .utf8), let o = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else {
            return .failed("The daemon's answer about the session could not be read.")
        }
        return (o["idle"] as? Bool ?? false) ? .launch(anyway: false) : .confirm
    }

    /// The argument vector, the CLI by its full path.
    public static func argv(cli: String, task: UInt64, anyway: Bool) -> [String] {
        [cli, "task", "open", String(task)] + (anyway ? ["--anyway"] : [])
    }

    /// The same as one line for a shell, quoting what needs it.
    public static func commandLine(cli: String, task: UInt64, anyway: Bool) -> String {
        argv(cli: cli, task: task, anyway: anyway).map(shellQuote).joined(separator: " ")
    }

    static func shellQuote(_ s: String) -> String {
        if !s.isEmpty, s.allSatisfy({ $0.isLetter || $0.isNumber || "/._-~=".contains($0) }) { return s }
        return "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    public static let confirmQuestion = "The step is running; a second session would collide with it. Open anyway?"

    public static func copiedNotice(_ line: String) -> String { "Copied `\(line)`; paste it in a terminal." }
}
