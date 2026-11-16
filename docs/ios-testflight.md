# iOS on TestFlight

`.github/workflows/ios-testflight.yml` archives the universal iOS app (`clients/swift/ios`) and
uploads it to TestFlight on every push to `develop` that touches `clients/swift/**`, or on a
manual run (Actions, "iOS TestFlight", Run workflow).

- `CFBundleShortVersionString` is the workspace version in `Cargo.toml`; `CFBundleVersion` is the run number.
- Without every input below the job builds unsigned for the simulator, uploads nothing and prints a `::warning::`.
- The API key is written to `$RUNNER_TEMP` with mode 0600, never echoed, and deleted in a final `always()` step.

## One-time steps for the owner

1. **Bundle id.** In the Apple Developer portal, Identifiers, register an App ID for the bundle id you will use
   (for example `dev.mason.ios`) with the **Push Notifications** capability.
2. **App record.** In App Store Connect, Apps, New App: iOS, that bundle id, any name and SKU.
3. **API key.** App Store Connect, Users and Access, Integrations, App Store Connect API: create a key with the
   **App Manager** role. Note the Key ID and Issuer ID and download the `.p8` (it can be downloaded once).
4. **Secrets and variable.** In the repository settings add secrets `ASC_KEY_ID`, `ASC_ISSUER_ID`,
   `ASC_KEY_P8` (the whole text of the `.p8` file), `APPLE_TEAM_ID` (the 10-character team id), and the
   repository **variable** `IOS_BUNDLE_ID`.
5. **Testers.** In App Store Connect, the app's TestFlight tab: add an internal testing group and its testers
   (they must be users of the team). Internal builds need no review.

Then run the workflow once by hand. The first real upload has not happened until these are added.
