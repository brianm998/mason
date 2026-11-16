// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI
#if canImport(VisionKit) && os(iOS)
import VisionKit
#endif

/// The daemons Bonjour has found, refreshed while the pairing screen is up.
@MainActor
public final class NearbyList: ObservableObject {
    @Published public private(set) var services: [ResolvedService] = []
    private let source: ServiceSource?
    private var timer: Timer?
    public init(source: ServiceSource?) { self.source = source }

    public func refresh() { services = (source?.services() ?? []).sorted { $0.record.instance < $1.record.instance } }
    public func start() {
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.refresh() }
        }
    }
    public func stop() { timer?.invalidate(); timer = nil }
}

/// Shown when no daemon is paired: scan the QR code, or pick a nearby daemon and type its code.
public struct PairingView: View {
    @ObservedObject var model: PairingModel
    @ObservedObject var nearby: NearbyList

    public init(model: PairingModel, nearby: NearbyList) { self.model = model; self.nearby = nearby }

    public var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Pair with your MASON daemon").font(.title2.bold())
            Picker("How", selection: Binding(get: { model.state.path }, set: { model.state.path = $0 })) {
                Text("Scan").tag(PairingScreenState.Path.scan)
                Text("Nearby").tag(PairingScreenState.Path.nearby)
            }
            .pickerStyle(.segmented)
            .accessibilityIdentifier("pair:path")
            switch model.state.path {
            case .scan: scan
            case .nearby: nearbyList
            }
            codeEntry
            if let e = model.state.error {
                Text(e).foregroundStyle(.red).accessibilityIdentifier("pair:error")
            }
            Button(model.state.busy ? "Pairing…" : "Pair") { Task { await model.submit() } }
                .buttonStyle(.borderedProminent)
                .disabled(!model.state.canSubmit)
                .accessibilityIdentifier("pair:submit")
            Spacer(minLength: 0)
        }
        .padding()
        .onAppear { nearby.start() }
        .onDisappear { nearby.stop() }
    }

    @ViewBuilder private var scan: some View {
        if let a = model.state.address {
            VStack(alignment: .leading, spacing: 4) {
                Text(a.host).font(.headline)
                Text("Fingerprint \(PairingScreenState.shortFingerprint(a.fingerprint))…").font(.footnote).foregroundStyle(.secondary)
                Text("From a QR code or link: the full fingerprint is pinned.").font(.footnote).foregroundStyle(.secondary)
            }
        } else {
            #if canImport(VisionKit) && os(iOS)
            QRScannerView { model.state.scanned($0) }
                .frame(minHeight: 240, maxHeight: 360)
                .clipShape(RoundedRectangle(cornerRadius: 12))
            #else
            Text("Scanning needs the camera on an iPhone or iPad. Open a mason://pair link or use Nearby.")
                .foregroundStyle(.secondary)
            #endif
        }
    }

    private var nearbyList: some View {
        VStack(alignment: .leading, spacing: 8) {
            if nearby.services.isEmpty {
                Text("Looking for daemons on this network…").foregroundStyle(.secondary)
            }
            ForEach(nearby.services, id: \.record.instance) { s in
                Button { model.state.choose(s) } label: {
                    HStack {
                        VStack(alignment: .leading) {
                            Text(s.record.instance).font(.headline)
                            Text("Fingerprint \(PairingScreenState.shortFingerprint(s.record.fingerprint))")
                                .font(.footnote.monospaced()).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if model.state.chosen?.record.instance == s.record.instance { Image(systemName: "checkmark") }
                    }
                }
                .buttonStyle(.bordered)
            }
            Text(PairingScreenState.nearbyWarning).font(.footnote).foregroundStyle(.secondary)
        }
    }

    private var codeEntry: some View {
        TextField("8-digit code", text: Binding(get: { model.state.code }, set: { model.state.setCode($0) }))
            #if os(iOS)
            .keyboardType(.numberPad)
            #endif
            .textFieldStyle(.roundedBorder)
            .accessibilityIdentifier("pair:code")
    }
}

/// The paired daemon, its role and the host last used, with Forget.
public struct PairedDaemonSettingsView: View {
    let record: PairedRecord?
    let host: String?
    let forget: () -> Void
    let push: PushRegistrar?
    public init(record: PairedRecord?, host: String?, push: PushRegistrar? = nil, forget: @escaping () -> Void) {
        self.record = record; self.host = host; self.push = push; self.forget = forget
    }

    public var body: some View {
        Form {
            if let r = record {
                Section("Paired daemon") {
                    LabeledContent("Name", value: r.host)
                    LabeledContent("Role", value: r.role)
                    LabeledContent("Fingerprint", value: PairingScreenState.shortFingerprint(r.fingerprint))
                    if let h = host { LabeledContent("Last used", value: h) }
                }
                if let push { PushSettingsSection(push: push) }
                Section {
                    Button("Forget this daemon", role: .destructive, action: forget)
                        .accessibilityIdentifier("settings:forget")
                }
            } else {
                Text("No daemon is paired.")
            }
        }
    }
}

#if canImport(VisionKit) && os(iOS)
/// The camera, reading QR codes only. Not tested (it needs a camera); the text it reads goes
/// through `PairingScreenState.scanned`.
struct QRScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> UIViewController {
        guard DataScannerViewController.isSupported, DataScannerViewController.isAvailable else {
            let l = UILabel()
            l.text = "The camera is not available. Use Nearby."
            l.textAlignment = .center
            l.numberOfLines = 0
            let vc = UIViewController()
            vc.view = l
            return vc
        }
        let s = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])],
                                          qualityLevel: .balanced, recognizesMultipleItems: false,
                                          isHighFrameRateTrackingEnabled: false, isHighlightingEnabled: true)
        s.delegate = context.coordinator
        try? s.startScanning()
        return s
    }

    func updateUIViewController(_ vc: UIViewController, context: Context) {}
    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onCode: (String) -> Void
        init(onCode: @escaping (String) -> Void) { self.onCode = onCode }
        func dataScanner(_ s: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for case let .barcode(b) in items {
                if let t = b.payloadStringValue { onCode(t); return }
            }
        }
    }
}
#endif

/// Settings > Notifications: on or off for this device, and a test push.
struct PushSettingsSection: View {
    @ObservedObject var push: PushRegistrar
    @State private var on = false

    var body: some View {
        Section("Notifications") {
            Toggle("Notify this device", isOn: Binding(get: { on }, set: { v in
                on = v
                Task { await push.setEnabled(v) }
            }))
            .accessibilityIdentifier("settings:notify")
            Button("Send a test") { Task { await push.sendTest() } }
                .disabled(!on)
                .accessibilityIdentifier("settings:notify-test")
            if let r = push.testResult { Text(r).font(.footnote).foregroundStyle(.secondary) }
        }
        .onAppear { on = push.enabled }
    }
}

/// Shown once, before the system asks for permission.
public struct PushExplainView: View {
    let answer: (Bool) -> Void
    public init(answer: @escaping (Bool) -> Void) { self.answer = answer }

    public var body: some View {
        VStack(spacing: 20) {
            Image(systemName: "bell.badge").font(.system(size: 48))
            Text("Get told when MASON needs you").font(.title2.bold()).multilineTextAlignment(.center)
            Text("MASON can send a notification when a task needs your decision, a task fails or a landing stops, or the daemon goes down. Tapping one opens that task. You can turn this off in Settings.")
                .multilineTextAlignment(.center)
            Button("Turn on notifications") { answer(true) }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("push:allow")
            Button("Not now") { answer(false) }
                .accessibilityIdentifier("push:decline")
        }
        .padding(32)
    }
}
