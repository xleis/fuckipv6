package cc.xlei.fuckipv6.android.bridge

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Turns a burst of SSE frames into at most one authoritative snapshot pull.
 *
 * The node emits a frame per change, and a single operation can produce several
 * in a row. Pulling the snapshot per frame would turn one user action into a
 * poll storm, so frames inside a short window collapse into one pull. A frame
 * whose cursor was already applied is a replay after a reconnect and is ignored
 * entirely.
 *
 * The coalescer also remembers the newest cursor so a reconnect can resume from
 * it instead of asking the node to replay everything.
 */
class SnapshotFrameCoalescer(
    private val scope: CoroutineScope,
    private val pull: () -> Unit,
    private val windowMillis: Long = 150,
) {
    private var resumeCursor: String? = null
    private var appliedCursor: String? = null
    private var scheduled: Job? = null

    /** Newest cursor observed, for `Last-Event-ID` / `?after=` on reconnect. */
    fun resumeCursor(): String? = resumeCursor

    /**
     * Records [frame] and schedules a pull when it demands one. Returns `true`
     * when this call scheduled a refresh.
     */
    fun onFrame(frame: NodeEventFrame): Boolean {
        val cursor = frame.cursor ?: frame.latestCursor
        if (cursor != null) {
            resumeCursor = cursor
        }
        if (frame.action() == LiveAction.Ignore) return false
        if (cursor != null && cursor == appliedCursor) return false
        scheduled?.cancel()
        scheduled = scope.launch {
            delay(windowMillis)
            appliedCursor = cursor ?: appliedCursor
            pull()
        }
        return true
    }
}
