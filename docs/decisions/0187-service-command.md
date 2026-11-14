# 0187: `mason service` writes a marked unit and drives the service manager through a runner

**Decision.** `mason service install` writes `~/Library/LaunchAgents/io.github.brianm998.masond.plist` or `~/.config/systemd/user/masond.service`, running the masond beside the running `mason` with `--managed` and absolute `--data-dir`/`--config-dir`, KeepAlive/Restart=always. Each unit contains the text "Written by `mason service install`"; a unit without it is never overwritten (without `--force`) or removed. `--print` writes the unit to stdout and touches nothing.

**Seam.** One install and one uninstall function per platform in `service.rs`, selected by `Kind`; Windows adds a third. All manager calls go through `Runner`.

**Doctor.** `check_service` reports whether a marked unit exists.

**Trust.** No PROTECTED paths.
