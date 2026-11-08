// SPDX-License-Identifier: GPL-3.0-or-later

package mason.sample

import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Test

/** Runs on the leased emulator or phone (`./gradlew connectedDebugAndroidTest` with ANDROID_SERIAL set). */
class DeviceTest {
    @Test
    fun theAppIsInstalledUnderItsOwnName() {
        val ctx = InstrumentationRegistry.getInstrumentation().targetContext
        assertEquals("mason.sample", ctx.packageName)
    }
}
