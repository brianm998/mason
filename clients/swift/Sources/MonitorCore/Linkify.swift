// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
import MasonProtocol

/// One run of text from `Linkify.segments`.
public enum LinkSegment: Equatable {
    /// Plain text.
    case text(String)
    /// A reference to a task the monitor knows: open its page.
    case task(id: UInt64, text: String)
    /// A page to open in the default browser.
    case url(url: String, text: String)

    public var text: String {
        switch self {
        case .text(let t), .task(_, let t), .url(_, let t): return t
        }
    }
}

/// Turns a string into plain text, task links and URL links. Pure, so tests cover it.
///
/// - `#N` is a task link only when `isKnownTask(N)` and the text before it does not make it
///   something else: `PR #N`, `pull request #N`, `Batch #N`, `debt #N`, `issue #N`, and the
///   `(#N)` of "the pull request's checks (#N)" are not tasks.
/// - A `CiLink`'s label (`PR #7`, a failed check's name) becomes a URL link when its URL is
///   http(s); the label of a link with another kind of URL stays plain text.
/// - A bare `http(s)://` URL becomes a URL link.
public enum Linkify {
    /// Words that, right before a `#N`, say it is not a task.
    static let notTask = ["pr", "pull request", "batch", "debt", "issue", "run"]

    public static func isWebURL(_ s: String) -> Bool {
        let l = s.lowercased()
        return (l.hasPrefix("https://") || l.hasPrefix("http://")) && l.count > 8
            && !s.contains(where: { $0.isWhitespace || $0.isNewline })
    }

    public static func segments(
        _ text: String, isKnownTask: (UInt64) -> Bool, ciLinks: [Mason_V1_CiLink] = []
    ) -> [LinkSegment] {
        let chars = Array(text)
        let labels = ciLinks.filter { !$0.label.isEmpty && isWebURL($0.url) }
            .sorted { $0.label.count > $1.label.count }
        var out: [LinkSegment] = []
        var plain = ""
        func flush() { if !plain.isEmpty { out.append(.text(plain)); plain = "" } }
        func isWord(_ c: Character) -> Bool { c.isLetter || c.isNumber || c == "_" }
        func matches(_ s: [Character], at i: Int) -> Bool {
            i + s.count <= chars.count && Array(chars[i..<i + s.count]) == s
        }
        var i = 0
        scan: while i < chars.count {
            let c = chars[i]
            let prevWord = i > 0 && isWord(chars[i - 1])
            // A CI label, exactly as the notes write it.
            if !prevWord {
                for l in labels {
                    let s = Array(l.label)
                    guard matches(s, at: i) else { continue }
                    let end = i + s.count
                    if end < chars.count, isWord(chars[end]), isWord(s[s.count - 1]) { continue }
                    flush()
                    out.append(.url(url: l.url, text: l.label))
                    i = end
                    continue scan
                }
            }
            // A bare URL.
            if (c == "h" || c == "H") && !prevWord, matches(Array("http"), at: i) || matches(Array("HTTP"), at: i) {
                var j = i
                while j < chars.count, !chars[j].isWhitespace, !"<>\"'".contains(chars[j]) { j += 1 }
                while j > i, ".,;:!?)]}".contains(chars[j - 1]) { j -= 1 }
                let s = String(chars[i..<j])
                if isWebURL(s) {
                    flush()
                    out.append(.url(url: s, text: s))
                    i = j
                    continue scan
                }
            }
            // A task number.
            if c == "#", !prevWord, i + 1 < chars.count, chars[i + 1].isASCII, chars[i + 1].isNumber {
                var j = i + 1
                while j < chars.count, chars[j].isASCII, chars[j].isNumber { j += 1 }
                let digits = String(chars[i + 1..<j])
                let before = String(chars[0..<i]).lowercased()
                let notATask = notTask.contains { w in
                    before.hasSuffix(w + " ") && (before.count == w.count + 1
                        || !isWord(Array(before)[before.count - w.count - 2]))
                } || before.hasSuffix("pull request's checks (")
                    || before.hasSuffix("(pr ") || before.hasSuffix("#")
                if !notATask, let n = UInt64(digits), isKnownTask(n) {
                    flush()
                    out.append(.task(id: n, text: "#" + digits))
                    i = j
                    continue scan
                }
                // "the pull request's checks (#7)": the pull request, when its URL is known.
                if before.hasSuffix("pull request's checks ("),
                   let l = labels.first(where: { $0.label == "PR #" + digits }) {
                    flush()
                    out.append(.url(url: l.url, text: "#" + digits))
                    i = j
                    continue scan
                }
                plain += "#" + digits
                i = j
                continue scan
            }
            plain.append(c)
            i += 1
        }
        flush()
        return out
    }
}
