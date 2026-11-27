// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.Command
import mason.v1.CommandResult
import mason.v1.ServerMessage
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** s1093: what `GET /review` says, and the bodies the toggle, the time and the frequency send. */
class ReviewSettingsTest {
    private val body = """{"settings":{"enabled":true,"at":"03:30","every_days":7,"model":"opus","effort":"medium","max_proposals":10,"changed_by":"owner","changed_at":1},"problem":null,"project":"mason","includes":["mason","web"],"running":false,"running_task":null,"next_run":1,"next_run_text":"2026-12-01 03:30","last":{"task":41,"completed_text":"03:41","filed":[42,43],"summary":"task #41: 2 filed (#42, #43)"},"last_status":"task #41: 2 filed (#42, #43)"}"""

    @Test fun theReviewParses() {
        val r = ReviewSettings.parse(body)
        assertEquals(true, r.enabled)
        assertEquals("03:30", r.at)
        assertEquals(7, r.everyDays)
        assertEquals("opus", r.model)
        assertEquals("medium", r.effort)
        assertEquals(41L, r.lastTask)
        assertEquals(listOf(42L, 43L), r.filed)
        assertEquals("Proposals filed: #42, #43", r.filedText)
        assertEquals("Next run: 2026-12-01 03:30", r.nextText)
        assertNull(r.runningTask)
        assertEquals("Every 7 days", r.everyText)
    }

    @Test fun neverRunRunningAndOffSaySo() {
        val never = ReviewSettings.parse("""{"settings":{"enabled":false},"project":"mason","last":null}""")
        assertNull(never.lastTask)
        assertEquals("Next run: none (off)", never.nextText)
        assertEquals("No proposals filed", never.filedText)
        assertEquals("Running now: #9", ReviewSettings.parse("""{"project":"mason","running_task":9}""").nextText)
        assertTrue(ReviewSettings.parse("{}").nextText.contains("No project of MASON's own"))
        assertEquals(ReviewSettings(), ReviewSettings.parse("not json"))
    }

    @Test fun theToggleTheTimeAndTheFrequencySendOneKeyEach() {
        assertEquals(mapOf("enabled" to false), Codec.json(SettingsPayload.reviewEnabled(false)))
        assertEquals(mapOf("at" to "03:30"), Codec.json(SettingsPayload.reviewAt("03:30")))
        assertEquals(mapOf("every_days" to 7.0), Codec.json(SettingsPayload.reviewEvery(7)))
        assertEquals(listOf(1, 7), ReviewSettings.PRESETS.map { it.second })
    }

    @Test fun theTimeMovesRoundTheClock() {
        val r = ReviewSettings(at = "00:00")
        assertEquals("23:00", r.atMoved(hours = -1))
        assertEquals("23:55", r.atMoved(minutes = -5))
        assertEquals("00:05", r.atMoved(minutes = 5))
        assertNull(ReviewSettings.minutes("24:00"))
        assertNull(ReviewSettings.minutes("9"))
    }

    @Test fun settingsLoadsTheReviewAndSendsEachChange() {
        val sent = mutableListOf<Command>()
        val m = MonitorModel()
        m.sender = { sent += it }
        fun lines() = sent.map { "${it.raw?.method} ${it.raw?.path} ${it.raw?.json}" }
        fun answer(id: String, ok: Boolean = true, json: String = "", error: String = "") =
            m.apply(ServerMessage(result = CommandResult(id = id, ok = ok, json = json, error = error)))

        m.loadSettings()
        assertTrue("GET /review " in lines(), lines().toString())
        answer("review", json = body)
        assertEquals("03:30", m.reviewSettings?.at)

        sent.clear()
        m.changeReviewSetting(SettingsPayload.reviewEnabled(false))
        m.changeReviewSetting(SettingsPayload.reviewAt("00:00"))
        m.changeReviewSetting(SettingsPayload.reviewEvery(1))
        m.runReview()
        assertEquals(listOf("/review", "/review", "/review", "/review/run"), sent.map { it.raw?.path })
        assertEquals(listOf("POST", "POST", "POST", "POST"), sent.map { it.raw?.method })
        assertEquals(mapOf("at" to "00:00"), Codec.json(sent[1].raw?.json.orEmpty()))

        answer("review-save", json = """{"settings":{"enabled":false,"at":"00:00","every_days":1},"project":"mason"}""")
        assertEquals(false, m.reviewSettings?.enabled)
        sent.clear()
        answer("review-save", ok = false, error = "`at` must be a time of day")
        assertEquals("`at` must be a time of day", m.settingsNotice)
        assertEquals(listOf("GET /review "), lines())
    }
}
