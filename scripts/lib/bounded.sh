# SPDX-License-Identifier: GPL-3.0-or-later
# bounded SECONDS CMD...: run CMD, kill it after SECONDS. Uses timeout or gtimeout when
# present; stock macOS has neither, so fall back to perl's alarm (perl ships with macOS).
# A command killed by the limit returns non-zero (124 from timeout, 142 from the fallback).
bounded() {
  local secs=$1; shift
  if command -v timeout >/dev/null 2>&1; then timeout "$secs" "$@"
  elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$secs" "$@"
  else perl -e 'my $s = shift; $SIG{ALRM} = sub { kill "KILL", $pid; exit 142 }; $pid = fork() // die; if (!$pid) { exec @ARGV or exit 127 } alarm $s; waitpid $pid, 0; my $st = $?; exit($st & 127 ? 128 + ($st & 127) : $st >> 8)' "$secs" "$@"
  fi
}
