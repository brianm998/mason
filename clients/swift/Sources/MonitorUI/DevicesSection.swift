// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// Settings > Devices (s433): whether LAN access is on, the paired devices with Revoke, and
/// "Pair a device…", which shows the `mason://pair` URL as a QR code with the 8-digit code.
struct DevicesSection: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    @State private var pairing = false
    @State private var revoking: PairedDevice?

    var body: some View {
        section("Devices", theme: theme) {
            if let n = model.devicesNotice {
                Text(n).font(theme.body()).foregroundStyle(theme.muted).accessibilityIdentifier("devices:notice")
            }
            if let lan = model.lanStatus {
                if lan.enabled {
                    Text("LAN access is on" + (lan.hosts.isEmpty ? "." : ": " + lan.hosts.joined(separator: ", ")))
                        .font(theme.body()).accessibilityIdentifier("devices:lan")
                    Button("Pair a device…") { pairing = true }.accessibilityIdentifier("devices:pair")
                } else {
                    Text(LanStatus.howToTurnOn).font(theme.body()).foregroundStyle(theme.muted)
                        .accessibilityIdentifier("devices:lan")
                }
            }
            if let list = model.devices {
                if list.active.isEmpty && list.revoked.isEmpty {
                    Text("No device is paired.").font(theme.body()).foregroundStyle(theme.muted)
                }
                ForEach(list.active, id: \.id) { d in
                    HStack(spacing: theme.spacing) {
                        Text("\(d.name) (\(d.role)), last seen \(DeviceList.lastSeenText(d.lastSeen, nowMs: model.daemonNow()))")
                            .font(theme.body())
                        Button("Revoke") { revoking = d }.accessibilityIdentifier("devices:revoke:\(d.id)")
                    }
                }
                if !list.revoked.isEmpty {
                    Text("Revoked").font(theme.small()).foregroundStyle(theme.muted)
                    ForEach(list.revoked, id: \.id) { d in
                        Text("\(d.name) (\(d.role)), revoked")
                            .font(theme.body()).foregroundStyle(theme.muted)
                    }
                }
            }
        }
        .alert("Revoke this device?", isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })) {
            Button("Revoke", role: .destructive) { if let d = revoking { model.revokeDevice(d.id) }; revoking = nil }
            Button("Cancel", role: .cancel) { revoking = nil }
        } message: {
            Text("\(revoking?.name ?? "It") will be signed out and must pair again.")
        }
        .sheet(isPresented: $pairing, onDismiss: { model.closePairing() }) { PairSheet(model: model, close: { pairing = false }) }
    }
}

/// The pairing sheet: a role, a QR code, the code in large type, the hosts and a countdown that
/// offers a new code when it runs out. While it is open the device list is re-read every 3 s so a
/// device that pairs appears.
struct PairSheet: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let close: () -> Void
    @State private var viewer = false
    @State private var now = Date()

    var body: some View {
        VStack(spacing: theme.spacingLarge) {
            Text("Pair a device").font(theme.title())
            Picker("Role", selection: $viewer) {
                Text("Owner").tag(false)
                Text("Viewer").tag(true)
            }.pickerStyle(.segmented).frame(width: 220).accessibilityIdentifier("devices:role")
            if let o = model.pairOffer {
                if let img = QRCode.image(for: o.url, side: 360) {
                    Image(decorative: img, scale: 1).interpolation(.none).resizable().frame(width: 240, height: 240)
                        .accessibilityIdentifier("devices:qr")
                }
                Text(o.code).font(.system(size: 40, weight: .bold, design: .monospaced)).textSelection(.enabled)
                    .accessibilityIdentifier("devices:code")
                Text(o.hosts.joined(separator: ", ")).font(theme.small()).foregroundStyle(theme.muted)
                switch o.state(nowMs: UInt64(now.timeIntervalSince1970 * 1000)) {
                case .valid(let s):
                    Text("Expires in \(PairOffer.clock(s))").font(theme.body()).accessibilityIdentifier("devices:countdown")
                case .expired:
                    Text("This code has expired.").font(theme.body()).foregroundStyle(theme.muted)
                    Button("New code") { model.issuePairCode(role: viewer ? "viewer" : "owner") }
                        .accessibilityIdentifier("devices:new-code")
                }
            } else {
                Button("Make a code") { model.issuePairCode(role: viewer ? "viewer" : "owner") }
                    .accessibilityIdentifier("devices:new-code")
            }
            if let n = model.devicesNotice { Text(n).font(theme.small()).foregroundStyle(theme.muted) }
            Button("Done") { close() }.keyboardShortcut(.defaultAction)
        }
        .padding(theme.spacingLarge)
        .frame(minWidth: 380)
        .onChange(of: viewer) { _, _ in model.closePairing() }
        .task {
            if model.pairOffer == nil { model.issuePairCode(role: viewer ? "viewer" : "owner") }
            var tick = 0
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_000_000_000)
                now = Date()
                tick += 1
                if tick % 3 == 0 { model.loadDevices() }
            }
        }
    }
}
