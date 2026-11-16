# 0463: iOS TestFlight workflow

Status: accepted

- A separate workflow from `ios.yml`. Reason: it needs secrets and a different trigger (push to `develop` only), and `ios.yml` must stay unsigned for pull requests.
- Signing is automatic through the App Store Connect API key, with no certificates in secrets. Reason: fewer secrets to keep, and `-allowProvisioningUpdates` makes Xcode create them.
- "Signed" needs all five inputs (four secrets and `IOS_BUNDLE_ID`); otherwise unsigned with a warning, as `release.yml` does. Reason: forks and the time before setup stay green.
- `CURRENT_PROJECT_VERSION` is the run number, so builds rise monotonically; `manageAppVersionAndBuildNumber` is false so the number stays ours.
