<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0484: Skiko native load in the Compose test JVM

Status: accepted

Context: on 12cfa63f the Compose app tests that render through Skia (EditorSnapshotTest,
MascotTest, TopBarTest) failed with `java.lang.UnsatisfiedLinkError` (MascotTest.kt:66 and :80,
TopBarTest.kt:59 in task 478's output). This session could not run the build directly, so the
full message text was not read; the cause is inferred from those frames.

Decision: the `test` task in clients/compose/app/build.gradle.kts passes
`--enable-native-access=ALL-UNNAMED` (as the desktop run does) and a `java.io.tmpdir` inside the
build directory, where Skiko extracts its library. No test is skipped and no Error is caught.
