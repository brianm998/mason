// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

@MainActor
final class SettingsModelTests: XCTestCase {
    func testTheProjectSettingsParseAndANonSecretTokenIsOnlyAReference() {
        let s = SettingsSnapshot(json: ##"{"project":"web","proposals":{"agent_normal":"queued","agent_blocks":"queued","outside_normal":"proposed","outside_blocks":"proposed","per_task":3,"per_day":20,"introspection":{"low_small":"queued","low_large":"proposed","high_small":"proposed","high_large":"proposed"}},"schedules":[{"name":"nightly","kind":"task","every":"daily","title":"Sweep"}],"tracker":{"kind":"github","interval_s":60,"label":"mason","import_as":"proposed","require_label":true,"token":"set in file, not shown"}}"##)
        XCTAssertEqual(s.project, "web")
        XCTAssertEqual(s.dispositions["agent_normal"], "queued")
        XCTAssertEqual(s.introspection["low_small"], "queued")
        XCTAssertEqual(s.perDay, 20)
        XCTAssertEqual(s.schedules, [.init(name: "nightly", kind: "task", every: "daily", title: "Sweep")])
        XCTAssertEqual(s.tracker?.intervalS, 60)
        XCTAssertEqual(s.tracker?.requireLabel, true)
        XCTAssertEqual(s.tracker?.token, "set in file, not shown")
        XCTAssertNil(SettingsSnapshot(json: ##"{"project":"web","tracker":null}"##).tracker)
        XCTAssertEqual(SettingsSnapshot(json: "not json"), SettingsSnapshot())
    }

    func testTheNotificationRoutesParse() {
        let n = NotifySettings(json: ##"{"error":null,"routing":[{"condition":"failed","rules":[{"channel":"monitor","after_s":0},{"channel":"sound","after_s":0}]},{"condition":"paused","rules":[{"channel":"monitor","after_s":0}]}],"channels":["monitor","sound","email"],"quiet":{"sound":"22:00-07:00"},"reminder_after_s":1800,"reminder_repeat_s":7200,"configured":{"email":false,"webhook":true}}"##)
        XCTAssertEqual(n.routes.count, 2)
        XCTAssertTrue(n.sends("failed", to: "sound"))
        XCTAssertFalse(n.sends("paused", to: "sound"))
        XCTAssertFalse(n.sends("nonsense", to: "sound"))
        XCTAssertEqual(n.quiet["sound"], "22:00-07:00")
        XCTAssertEqual(n.reminderAfterS, 1800)
        XCTAssertEqual(n.configured["webhook"], true)
    }

    private func object(_ s: String) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: Data(s.utf8))) as? [String: Any] ?? [:]
    }

    func testEachPayloadIsOneSmallChange() {
        XCTAssertEqual(object(SettingsPayload.disposition(key: "agent_normal", value: "queued")) as NSDictionary,
                       ["proposals": ["agent_normal": "queued"]] as NSDictionary)
        XCTAssertEqual(object(SettingsPayload.introspection(key: "high_large", value: "queued")) as NSDictionary,
                       ["proposals": ["introspection": ["high_large": "queued"]]] as NSDictionary)
        XCTAssertEqual(object(SettingsPayload.cap(key: "per_day", value: 5)) as NSDictionary,
                       ["proposals": ["per_day": 5]] as NSDictionary)
        XCTAssertEqual(object(SettingsPayload.schedule(name: "nightly", every: "6h")) as NSDictionary,
                       ["schedules": ["nightly": ["every": "6h"]]] as NSDictionary)
        XCTAssertEqual(object(SettingsPayload.tracker(key: "require_label", value: true)) as NSDictionary,
                       ["tracker": ["require_label": true]] as NSDictionary)
        XCTAssertEqual(object(SettingsPayload.route(condition: "failed", channel: "email", delay: "5m")) as NSDictionary,
                       ["routing": ["failed": ["email": "5m"]]] as NSDictionary)
        // Turning a channel off sends null, which removes it from the file.
        XCTAssertEqual(object(SettingsPayload.route(condition: "failed", channel: "email", delay: nil)) as NSDictionary,
                       ["routing": ["failed": ["email": NSNull()]]] as NSDictionary)
        XCTAssertEqual(object(SettingsPayload.runSchedule(project: "web", name: "nightly")) as NSDictionary,
                       ["name": "web/nightly"] as NSDictionary)
        XCTAssertEqual(SettingsPayload.settingsPath(project: "web"), "/projects/web/settings")
    }

    private static let remoteBody = ##"{"mode":"favor-remote","default":"local","problem":"unknown mode `x`, treated as local","changed_by":"owner","changed_at":"2026-11-21T10:00:00Z","projects":[{"project":"web","remote":true,"runs_on":"macos-latest","mode":"remote","source":"project"},{"project":"api","remote":true,"runs_on":"ubuntu-latest","mode":"favor-remote","source":"global"},{"project":"local-only","remote":false,"runs_on":"","mode":"local","source":"no [remote_runs]"}]}"##

    func testTheRemoteStatusParsesInThreeSourceStates() {
        let r = RemoteSettings(json: Self.remoteBody)
        XCTAssertEqual(r.mode, "favor-remote")
        XCTAssertEqual(r.problem, "unknown mode `x`, treated as local")
        XCTAssertEqual(r.changedBy, "owner")
        XCTAssertEqual(r.changedText, "Changed by owner at 2026-11-21T10:00:00Z")
        XCTAssertEqual(r.projects.count, 3)
        let web = r.project("web")!
        XCTAssertEqual([web.remote, web.runsOn, web.mode, web.source], [true, "macos-latest", "remote", "project"] as [AnyHashable])
        XCTAssertEqual(web.sourceText, "set for this project")
        XCTAssertTrue(web.canChange)
        XCTAssertEqual(r.project("api")?.sourceText, "follows the global mode")
        let none = r.project("local-only")!
        XCTAssertFalse(none.remote)
        XCTAssertEqual(none.sourceText, "always runs here: the project file has no [remote_runs]")
        XCTAssertFalse(none.canChange)
        XCTAssertEqual(RemoteSettings(json: "not json"), RemoteSettings())
    }

    func testTheModeChangesBuildTheirBodiesAndARefusalIsTheNotice() {
        let m = MonitorModel()
        m.project = "web"
        var sent: [Mason_V1_Command] = []
        m.sender = { sent.append($0) }
        m.loadSettings()
        XCTAssertTrue(sent.contains { $0.id == "remote" && $0.raw.method == "GET" && $0.raw.path == "/remote" })
        sent = []
        m.changeGlobalMode("remote")
        m.changeProjectMode("favor-local")
        XCTAssertEqual(sent.map(\.id), ["remote-save", "remote-save"])
        XCTAssertEqual(sent.map(\.raw.path), ["/remote", "/remote"])
        XCTAssertEqual(object(sent[0].raw.json) as NSDictionary, ["mode": "remote"] as NSDictionary)
        XCTAssertEqual(object(sent[1].raw.json) as NSDictionary, ["mode": "favor-local", "project": "web"] as NSDictionary)
        var r = Mason_V1_CommandResult()
        r.id = "remote-save"; r.ok = false; r.error = "project `web` has no [remote_runs]"
        var sm = Mason_V1_ServerMessage()
        sm.result = r
        m.apply(sm)
        XCTAssertEqual(m.settingsNotice, "project `web` has no [remote_runs]")
        // An answer that worked reloads /remote.
        sent = []
        r.ok = true; r.error = ""
        sm.result = r
        m.apply(sm)
        XCTAssertEqual(sent.map(\.raw.path), ["/remote"])
    }

    func testThePageIsAnAddressAndThePageHasAPlaceInTheSidebar() {
        XCTAssertEqual(Page.settings.url.absoluteString, "mason://settings")
        XCTAssertEqual(Page.from(url: URL(string: "mason://settings")!), .settings)
        XCTAssertNil(Page.from(url: URL(string: "mason://settings/x")!))
    }
}
