// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class SettingsModelTest {
    @Test fun theProjectSettingsParseAndATokenIsOnlyAReference() {
        val s = SettingsSnapshot.parse("""{"project":"web","proposals":{"agent_normal":"queued","agent_blocks":"queued","outside_normal":"proposed","outside_blocks":"proposed","per_task":3,"per_day":20,"introspection":{"low_small":"queued","low_large":"proposed","high_small":"proposed","high_large":"proposed"}},"schedules":[{"name":"nightly","kind":"task","every":"daily","title":"Sweep"}],"tracker":{"kind":"github","interval_s":60,"label":"mason","import_as":"proposed","require_label":true,"token":"set in file, not shown"}}""")
        assertEquals("web", s.project)
        assertEquals("queued", s.dispositions["agent_normal"])
        assertEquals("queued", s.introspection["low_small"])
        assertEquals(20, s.perDay)
        assertEquals(listOf(SettingsSnapshot.Schedule("nightly", "task", "daily", "Sweep")), s.schedules)
        assertEquals(60, s.tracker?.intervalS)
        assertEquals(true, s.tracker?.requireLabel)
        assertEquals("set in file, not shown", s.tracker?.token)
        assertNull(SettingsSnapshot.parse("""{"project":"web","tracker":null}""").tracker)
        assertEquals(SettingsSnapshot(), SettingsSnapshot.parse("not json"))
    }

    @Test fun theNotificationRoutesParse() {
        val n = NotifySettings.parse("""{"error":null,"routing":[{"condition":"failed","rules":[{"channel":"monitor","after_s":0},{"channel":"sound","after_s":0}]},{"condition":"paused","rules":[{"channel":"monitor","after_s":0}]}],"channels":["monitor","sound","email"],"quiet":{"sound":"22:00-07:00"},"reminder_after_s":1800,"reminder_repeat_s":7200,"configured":{"email":false,"webhook":true}}""")
        assertEquals(2, n.routes.size)
        assertTrue(n.sends("failed", "sound"))
        assertFalse(n.sends("paused", "sound"))
        assertFalse(n.sends("nonsense", "sound"))
        assertEquals("22:00-07:00", n.quiet["sound"])
        assertEquals(1800, n.reminderAfterS)
        assertEquals(true, n.configured["webhook"])
    }

    @Test fun eachPayloadIsOneSmallChange() {
        assertEquals(mapOf("proposals" to mapOf("agent_normal" to "queued")), Codec.json(SettingsPayload.disposition("agent_normal", "queued")))
        assertEquals(mapOf("proposals" to mapOf("introspection" to mapOf("high_large" to "queued"))), Codec.json(SettingsPayload.introspection("high_large", "queued")))
        assertEquals(mapOf("proposals" to mapOf("per_day" to 5.0)), Codec.json(SettingsPayload.cap("per_day", 5)))
        assertEquals(mapOf("schedules" to mapOf("nightly" to mapOf("every" to "6h"))), Codec.json(SettingsPayload.schedule("nightly", "6h")))
        assertEquals(mapOf("tracker" to mapOf("require_label" to true)), Codec.json(SettingsPayload.tracker("require_label", true)))
        assertEquals(mapOf("routing" to mapOf("failed" to mapOf("email" to "5m"))), Codec.json(SettingsPayload.route("failed", "email", "5m")))
        // Turning a channel off sends null, which removes it from the file.
        val off = SettingsPayload.route("failed", "email", null)
        assertTrue(off.contains("\"email\":null"), off)
        assertEquals(mapOf("name" to "web/nightly"), Codec.json(SettingsPayload.runSchedule("web", "nightly")))
        assertEquals("/projects/web/settings", SettingsPayload.path("web"))
    }

    @Test fun thePageIsAnAddress() {
        assertEquals("mason://settings", Page.Settings.url)
        assertEquals(Page.Settings, Page.from("mason://settings"))
        assertNull(Page.from("mason://settings/x"))
    }
}
