// SPDX-License-Identifier: GPL-3.0-or-later

/// Which banner image goes with which banner, chosen only from the banner's own identity:
/// the daemon's `Banner.kind` or the client's own banner. An unknown kind has no image.
public enum BannerIcons {
    /// Every `Banner.kind` the daemon emits today, and the image id it gets (nil: deliberately none).
    public static let daemonKinds: [String: String?] = [
        "paused": "paused",
        "definition": "definition",
        "landing_frozen": "landing_frozen",
        "provider_outage": "provider",
        "provider_overage": "provider",
        "provider_budget": "provider",
        "provider_hold": "provider",
        "provider_limit": "provider",
        // Informational notes, not trouble: no image.
        "provider_extra_billing": nil,
        "provider_warning": nil,
    ]

    /// The client's own banners, by their accessibility identifiers.
    public static let clientBanners: [String: String] = [
        "connection": "connection_lost",
        "daemon-down": "daemon_down",
        "read-only": "read_only",
        "monitor-update": "update",
    ]

    public static func id(forDaemonKind kind: String) -> String? {
        daemonKinds[kind] ?? nil
    }
}
