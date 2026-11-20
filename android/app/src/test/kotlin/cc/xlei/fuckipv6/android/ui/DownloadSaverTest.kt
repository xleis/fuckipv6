package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.DownloadFetcher
import cc.xlei.fuckipv6.android.bridge.FetchOutcome
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.OutputStream

class DownloadSaverTest {
    private class RecordingSink : DownloadSink {
        val stream = ByteArrayOutputStream()
        var closed = 0
        var discarded = 0
        var failOnClose = false

        override fun open(): OutputStream = stream

        override fun close() {
            if (failOnClose) throw IOException("close failed")
            closed += 1
        }

        override fun discard() {
            discarded += 1
        }
    }

    private fun saver(fetcher: DownloadFetcher) =
        DownloadSaver(fetcher, CoroutineScope(Dispatchers.Default))

    private fun await(saver: DownloadSaver, predicate: (DownloadState) -> Boolean): DownloadState {
        val deadline = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < deadline) {
            val state = saver.state().value
            if (predicate(state)) return state
            Thread.sleep(5)
        }
        return saver.state().value
    }

    @Test
    fun completeStreamIsClosedAndReportedWithItsByteCount() {
        val sink = RecordingSink()
        val saver = saver { token, out, max ->
            out.write(ByteArray(10) { it.toByte() })
            FetchOutcome.Ok(10)
        }
        saver.save("tok", sink)
        val state = await(saver) { it is DownloadState.Saved }
        assertEquals(DownloadState.Saved(10), state)
        assertEquals(1, sink.closed)
        assertEquals(0, sink.discarded)
        assertFalse(sink.stream.size() == 0)
    }

    @Test
    fun failedFetchDiscardsTheSinkAndNeverClaimsSavedBytes() {
        val sink = RecordingSink()
        val saver = saver { _, _, _ -> FetchOutcome.Failed(BridgeFailure.HttpStatus(413)) }
        saver.save("tok", sink)
        val state = await(saver) { it is DownloadState.Failed }
        assertEquals("file.page.error.download_too_large_detail", (state as DownloadState.Failed).errorKey)
        assertEquals(0, sink.closed)
        assertEquals(1, sink.discarded)
    }

    @Test
    fun aCloseFailureBecomesAFailureInsteadOfSavedBytes() {
        val sink = RecordingSink().apply { failOnClose = true }
        val saver = saver { _, out, _ ->
            out.write(ByteArray(4))
            FetchOutcome.Ok(4)
        }
        saver.save("tok", sink)
        val state = await(saver) { it is DownloadState.Failed }
        assertEquals("file.page.error.generic", (state as DownloadState.Failed).errorKey)
        assertEquals(1, sink.discarded)
    }

    @Test
    fun openFailureIsReportedWithoutTouchingDiscardOnUnopenedSink() {
        val sink = object : DownloadSink {
            override fun open(): OutputStream = throw IOException("cannot open document")
            override fun close() = Unit
            override fun discard() = throw AssertionError("discard must not run for an unopened sink")
        }
        val saver = saver { _, _, _ -> FetchOutcome.Ok(0) }
        saver.save("tok", sink)
        val state = await(saver) { it is DownloadState.Failed }
        assertEquals("file.page.error.generic", (state as DownloadState.Failed).errorKey)
    }

    @Test
    fun emptyTokenAndResetKeepTheStateHonest() {
        val saver = saver { _, _, _ -> FetchOutcome.Ok(0) }
        saver.save("   ", RecordingSink())
        val state = await(saver) { it is DownloadState.Failed }
        assertTrue(state is DownloadState.Failed)
        saver.reset()
        assertEquals(DownloadState.Idle, saver.state().value)
    }
}
