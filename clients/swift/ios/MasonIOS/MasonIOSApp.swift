// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import MonitorPhone
import MonitorUI
import SwiftUI
import UIKit

/// The iOS app, one universal build for iPhone and iPad. The Xcode project is generated from
/// `ios/project.yml` by XcodeGen (`make ios`); it is not committed.
@main
struct MasonIOSApp: App {
    @StateObject private var model: MonitorModel
    @StateObject private var session: RemoteSession
    @StateObject private var pairing: PairingModel
    @StateObject private var nearby: NearbyList
    @Environment(\.scenePhase) private var phase

    init() {
        let model = MonitorModel(preferences: DefaultsPreferences())
        let store = DefaultsPairedStore()
        let tokens = KeychainTokenStore()
        let source = BonjourServiceSource()
        source.start()
        let session = RemoteSession(model: model, store: store, tokens: tokens, source: source)
        _model = StateObject(wrappedValue: model)
        _session = StateObject(wrappedValue: session)
        _pairing = StateObject(wrappedValue: PairingModel(session: session, store: store, tokens: tokens, deviceName: UIDevice.current.name))
        _nearby = StateObject(wrappedValue: NearbyList(source: source))
    }

    var body: some Scene {
        WindowGroup {
            Group {
                switch session.state {
                case .unpaired: PairingView(model: pairing, nearby: nearby)
                case .paired: AppRoot(model: model, session: session, pairing: pairing)
                }
            }
            // A mason://pair link (the camera app, a message) opens the pairing screen filled in.
            .onOpenURL { url in
                if session.state == .unpaired { pairing.open(url: url) }
            }
        }
        .onChange(of: phase, initial: true) { _, p in
            if p == .active { session.activate() } else if p == .background { session.deactivate() }
        }
    }
}

/// Picks the layout by horizontal size class: regular is the iPad root (the shared
/// `RootView` until the iPad layout exists), compact is the phone's. A gear opens Settings.
struct AppRoot: View {
    @ObservedObject var model: MonitorModel
    let session: RemoteSession
    let pairing: PairingModel
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var settings = false

    var body: some View {
        Group {
            switch RootChoice.choose(regularWidth: sizeClass.map { $0 == .regular }) {
            case .pad: RootView(model: model)
            case .phone: PhoneRootView(model: model)
            }
        }
        .overlay(alignment: .bottomTrailing) {
            Button { settings = true } label: { Image(systemName: "gearshape").padding(12) }
                .background(.regularMaterial, in: Circle())
                .padding()
                .accessibilityLabel("Daemon settings")
        }
        .sheet(isPresented: $settings) {
            NavigationStack {
                PairedDaemonSettingsView(record: session.paired.map(PairedRecord.init), host: model.linkStatus) {
                    settings = false
                    pairing.forget()
                }
                .navigationTitle("Settings")
            }
        }
    }
}
