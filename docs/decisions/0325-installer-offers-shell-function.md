# 0325: the installer may add a `mason()` shell function, on an explicit yes

**Decision.** Stage 26 said the installer never edits the shell profile. That is amended: `scripts/install.sh` appends (or replaces in place) one block between `# >>> mason >>>` and `# <<< mason <<<` holding a `mason()` function, but only when the owner answers yes to a question naming the file, or passes `--profile`. Non-interactive runs, `--no-profile`, `--yes` alone and "n" edit nothing. Nothing outside the markers is ever touched.

**Why.** Release installs, `~/.config/mason/bin` and self-updated runtimes leave several `mason` binaries, and the one on PATH is often not the one the daemon runs. A function that follows `runtimes/current` never goes stale. The profile file is the one the login shell reads (zsh `~/.zprofile`; bash `~/.bash_profile` if it exists, since it hides `~/.profile`; else `~/.profile`).

**Not done.** `mason init` and `mason doctor` do not offer it; only the install script does.
