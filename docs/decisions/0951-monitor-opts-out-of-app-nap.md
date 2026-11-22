# 0951: The monitor opts out of App Nap

Stage s958. macOS naps an app whose window is not frontmost and runs its threads at priority 4,
below every compile thread on a loaded build machine. The monitor's work is small (about 1.5 s of
CPU a minute) but waited seconds for a core, and it is what speaks alerts.

Decision: the monitor takes one process activity with `.userInitiatedAllowingIdleSystemSleep` and
`.latencyCritical` at start and never ends it (`AppActivity`, injectable for tests). Not
`.idleDisplaySleepDisabled`: the display must still sleep. The bundle's Info.plist also gets
`NSAppSleepDisabled`, but the bare binary the owner runs has no bundle, so the code is the
mechanism. Napping saves nothing on a desktop on mains power.

Not verified: the live thread priority behind another app.
