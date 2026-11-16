// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import MonitorPhone
import MonitorUI
import SwiftUI
import UIKit
import UserNotifications

/// Receives the push token and notification taps, which UIKit delivers only to a delegate. The
/// app hands it what to call in `MasonIOSApp.init`.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    @MainActor static var push: PushRegistrar?
    @MainActor static var model: MonitorModel?

    func application(_ application: UIApplication, didFinishLaunchingWithOptions o: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        Task { @MainActor in await Self.push?.didRegister(deviceToken: token) }
    }

    /// In the foreground: a banner only when the item is not already on screen.
    func userNotificationCenter(_ c: UNUserNotificationCenter, willPresent n: UNNotification) async -> UNNotificationPresentationOptions {
        await MainActor.run {
            guard let p = PushPayload(userInfo: n.request.content.userInfo), let m = Self.model else { return [.banner, .sound] }
            return p.showsBanner(whileShowing: m.page) ? [.banner, .sound] : []
        }
    }

    /// A tap opens the task's page, or Home.
    func userNotificationCenter(_ c: UNUserNotificationCenter, didReceive r: UNNotificationResponse) async {
        await MainActor.run {
            if let p = PushPayload(userInfo: r.notification.request.content.userInfo) { Self.model?.go(p.page) }
        }
    }
}

/// The iOS app, one universal build for iPhone and iPad. The Xcode project is generated from
/// `ios/project.yml` by XcodeGen (`make ios`); it is not committed.
@main
struct MasonIOSApp: App {
    @StateObject private var model: MonitorModel
    @StateObject private var session: RemoteSession
    @StateObject private var pairing: PairingModel
    @StateObject private var nearby: NearbyList
    @StateObject private var push: PushRegistrar
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @Environment(\.scenePhase) private var phase

    init() {
        let model = MonitorModel(preferences: DefaultsPreferences(), defaultTextScale: PlatformFeatures.iPadTextScale)
        let store = DefaultsPairedStore()
        let tokens = KeychainTokenStore()
        let source = BonjourServiceSource()
        source.start()
        let session = RemoteSession(model: model, store: store, tokens: tokens, source: source)
        let push = PushRegistrar(
            prefs: DefaultsPushPrefs(),
            transport: { session.paired.map { PinnedPushTransport(daemon: $0) } },
            fingerprint: { session.paired?.fingerprint },
            requestRegistration: {
                UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { ok, _ in
                    if ok { DispatchQueue.main.async { UIApplication.shared.registerForRemoteNotifications() } }
                }
            })
        session.push = push
        AppDelegate.push = push
        AppDelegate.model = model
        _push = StateObject(wrappedValue: push)
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
                case .paired: AppRoot(model: model, session: session, pairing: pairing, push: push)
                }
            }
            .sheet(isPresented: Binding(get: { push.needsExplanation && session.state == .paired }, set: { _ in })) {
                PushExplainView { push.explanationAnswered(allow: $0) }.interactiveDismissDisabled()
            }
            .onChange(of: session.state, initial: true) { _, s in
                if s == .paired { push.launched() }
            }
            // A mason://pair link (the camera app, a message) opens the pairing screen filled in.
            .onOpenURL { url in
                if session.state == .unpaired { pairing.open(url: url) }
            }
        }
        .onChange(of: phase, initial: true) { _, p in
            if p == .active { session.activate(); Task { await push.sendIfChanged() } } else if p == .background { session.deactivate() }
        }
    }
}

/// Picks the layout by horizontal size class: regular is the iPad root (the shared
/// `RootView` until the iPad layout exists), compact is the phone's. A gear opens Settings.
struct AppRoot: View {
    @ObservedObject var model: MonitorModel
    let session: RemoteSession
    let pairing: PairingModel
    let push: PushRegistrar
    @Environment(\.horizontalSizeClass) private var sizeClass
    @State private var settings = false

    var body: some View {
        Group {
            switch RootChoice.choose(regularWidth: sizeClass.map { $0 == .regular }) {
            case .pad: RootView(model: model, features: .iPad)
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
                PairedDaemonSettingsView(record: session.paired.map(PairedRecord.init), host: model.linkStatus, push: push) {
                    settings = false
                    pairing.forget()
                }
                .navigationTitle("Settings")
            }
        }
    }
}
