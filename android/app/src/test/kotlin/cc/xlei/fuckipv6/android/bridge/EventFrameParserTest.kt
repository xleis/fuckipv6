package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class EventFrameParserTest {
    @Test
    fun snapshotFrameCarriesTheAuthoritativeSnapshotAndCursor() {
        val line = """data: {"type":"snapshot","schema_version":1,"request_id":3,"server_time":"t",""" +
            """"cursor":{"sequence":5},"snapshot":{"revision":5,"file_capability":{"state":"enabled"}}}"""
        val frame = assertNotNull(EventFrameParser.parse(line)).let { EventFrameParser.parse(line)!! }
        assertEquals("snapshot", frame.type)
        assertEquals("5", frame.cursor)
        assertEquals("enabled", frame.payload?.getJSONObject("file_capability")?.getString("state"))
        val action = frame.action() as LiveAction.ApplySnapshot
        val parsed = EventFrameParser.parseSnapshot(action.snapshot).getOrThrow()
        assertEquals("status.capability.enabled", parsed.file.toPageData().stateKey)
    }

    @Test
    fun changeAndResyncFramesAskForKeyASnapshotWithoutInventingOne() {
        val change = EventFrameParser.parse("""data: {"type":"event","event":{"kind":"peer","sequence":6}}""")!!
        assertEquals(LiveAction.RefreshSnapshot, change.action())
        assertNull(change.payload)

        val resync = EventFrameParser.parse(
            """data: {"type":"resync_required","requested_cursor":{"sequence":2},"latest_cursor":{"sequence":9}}""",
        )!!
        assertEquals("9", resync.latestCursor)
        assertEquals(LiveAction.Resync, resync.action())
    }

    @Test
    fun nonDataAndUnsupportedLinesAreIgnored() {
        for (line in listOf(
            "",
            "   ",
            ": keep-alive",
            "id: 12",
            "event: message",
            "retry: 3000",
            """data: {"type":"something_new"}""",
            "data: not json",
            "data:",
        )) {
            assertNull(line, EventFrameParser.parse(line))
        }
    }

    @Test
    fun snapshotFrameWithoutPayloadFallsBackToRefresh() {
        val frame = EventFrameParser.parse("""data: {"type":"snapshot","cursor":{"sequence":1}}""")!!
        assertEquals(LiveAction.RefreshSnapshot, frame.action())
    }

    @Test
    fun unknownTypesInsideTheStreamKeepParsing() {
        val frame = EventFrameParser.parse("""data: {"type":"event","event":{"kind":"future"}}""")!!
        assertEquals("event", frame.type)
        assertTrue(frame.action() is LiveAction.RefreshSnapshot)
    }
}
