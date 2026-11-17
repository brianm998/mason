// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.assertIsOff
import androidx.compose.ui.test.assertIsOn
import androidx.compose.ui.test.getBoundsInRoot
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.runComposeUiTest
import androidx.compose.ui.unit.dp
import mason.monitor.MonitorModel
import mason.v1.CpuHistory
import mason.v1.CpuPoint
import mason.v1.CpuShare
import mason.v1.CpuUsage
import mason.v1.DiskHistory
import mason.v1.DiskPoint
import mason.v1.DiskReading
import mason.v1.DiskVolume
import mason.v1.View
import kotlin.test.Test
import kotlin.test.assertTrue

/** s562: the disk section's controls and its place beside the CPU section. Offscreen: nothing is shown on the Mac. */
@OptIn(ExperimentalTestApi::class)
class DiskSectionTest {
    private val sp = DiskVolume(id = "a", name = "sp", mount = "/Volumes/sp", roles = listOf("data", "worktrees"))
    private val hd = DiskVolume(id = "b", name = "Macintosh HD", mount = "/", roles = listOf("cache"))

    private fun history(vararg volumes: DiskVolume) = DiskHistory(
        span_ms = 3_600_000, step_ms = 5000, volumes = volumes.toList(),
        points = listOf(1000L, 6000L, 11000L).map { t ->
            DiskPoint(at_ms = t, readings = volumes.map { DiskReading(volume = it.id, free_bytes = 400_000_000_000 + t, total_bytes = 1_800_000_000_000) })
        },
    )

    private fun model(vararg volumes: DiskVolume) = MonitorModel().also { m ->
        m.apply(View(disk_history = history(*volumes)))
        val now = System.currentTimeMillis()
        m.cpu = CpuUsage(
            busy_permille = 500, cores = 18, sampled_ms = now,
            shares = listOf(CpuShare(task = 7, kind = "task", label = "#7", title = "A very long task title that has to be cut short", project = "mason", permille = 300)),
        )
        m.cpuHistory = CpuHistory(
            span_ms = 3_600_000,
            points = listOf(1000L, 6000L).map { CpuPoint(at_ms = it, shares = listOf(CpuShare(task = 7, kind = "task", label = "#7", permille = 300))) },
        )
    }

    @Test fun checkboxesAppearForTwoVolumesAndNotForOne() = runComposeUiTest {
        val two = model(sp, hd)
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { DiskSection(two) } }
        onNodeWithTag("disk-check:sp").assertIsOn()
        onNodeWithTag("disk-check:Macintosh HD").assertIsOn()
        onNodeWithTag("disk-chart").assertExists()
    }

    @Test fun oneVolumeHasNoCheckboxes() = runComposeUiTest {
        val one = model(sp)
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { DiskSection(one) } }
        onNodeWithTag("disk-legend:sp").assertExists()
        onNodeWithTag("disk-check:sp").assertDoesNotExist()
        onNodeWithTag("disk-chart").assertExists()
    }

    @Test fun uncheckingRemovesTheLineButKeepsTheLegendRow() = runComposeUiTest {
        val m = model(sp, hd)
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { DiskSection(m) } }
        onNodeWithTag("disk-check:Macintosh HD").performClick()
        waitForIdle()
        onNodeWithTag("disk-check:Macintosh HD").assertIsOff()
        onNodeWithTag("disk-legend:Macintosh HD").assertExists()
        assertTrue("b" in m.hiddenVolumes)
        // The last shown volume off: no empty frame.
        onNodeWithTag("disk-check:sp").performClick()
        waitForIdle()
        onNodeWithTag("disk-none").assertExists()
        onNodeWithTag("disk-chart").assertDoesNotExist()
        onNodeWithTag("disk-legend:sp").assertExists()
    }

    @Test fun spanPickerAsksForAnotherRange() = runComposeUiTest {
        val m = model(sp)
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { DiskSection(m) } }
        onNodeWithTag("disk-span:24 h").performClick()
        waitForIdle()
        assertTrue(m.diskSpan.label == "24 h")
        onNodeWithTag("disk-collecting").assertExists()
    }

    @Test fun diskSitsRightOfCpuWhenWideAndBelowWhenNarrow() = runComposeUiTest {
        val m = model(sp, hd)
        var boxWidth by mutableStateOf(1000)
        setContent {
            CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) {
                Box(Modifier.width(boxWidth.dp)) { CpuDiskRow(m) }
            }
        }
        val cpu = onNodeWithTag("section-title:CPU (all projects)").getBoundsInRoot()
        val disk = onNodeWithTag("section-title:Disk (all projects)").getBoundsInRoot()
        assertTrue(disk.left > cpu.right, "disk is right of cpu: $cpu $disk")
        assertTrue(kotlin.math.abs(disk.top.value - cpu.top.value) < 1f)
        val cpuCol = onNodeWithTag("home:cpu-column").getBoundsInRoot()
        val diskCol = onNodeWithTag("home:disk-column").getBoundsInRoot()
        assertTrue(cpuCol.width > diskCol.width)
        // The CPU section keeps its legend and chart at the narrower width.
        onNodeWithTag("cpu-legend:#7").assertExists()
        onNodeWithTag("cpu-chart").assertExists()
        onNodeWithTag("disk-chart").assertExists()

        boxWidth = 600
        waitForIdle()
        val cpu2 = onNodeWithTag("section-title:CPU (all projects)").getBoundsInRoot()
        val disk2 = onNodeWithTag("section-title:Disk (all projects)").getBoundsInRoot()
        assertTrue(disk2.top > cpu2.bottom, "disk is below cpu: $cpu2 $disk2")
        onNodeWithTag("cpu-legend:#7").assertExists()
        onNodeWithTag("cpu-chart").assertExists()
    }

    @Test fun anOlderDaemonHidesTheDiskSection() = runComposeUiTest {
        val m = MonitorModel()
        setContent { CompositionLocalProvider(LocalTheme provides Theme(false, 1.0)) { DiskSection(m) } }
        onNodeWithTag("section-title:Disk (all projects)").assertDoesNotExist()
    }
}
