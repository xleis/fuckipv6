package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.android.ui.FilePageData
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Everything the Android surfaces can know about the node.
 *
 * `NotConfigured` is distinct from `Unavailable`: a node URL that was never set
 * must not look like a network problem, and a failed poll must never keep
 * showing the previous snapshot as if it were current.
 */
sealed interface BridgeState {
    data object NotConfigured : BridgeState
    data class Ready(val data: FilePageData, val snapshotRevision: Long?) : BridgeState
    data class Failed(val failure: BridgeFailure) : BridgeState

    val isReady: Boolean
        get() = this is Ready
}

/**
 * Polls the node snapshot and publishes the projected File state.
 *
 * The bridge is intentionally dumb: it has no retry-with-backoff that could
 * hide the control plane, no optimistic writes, and no command channel yet.
 * Transfers stay on the desktop surfaces until the node command protocol has an
 * Android transport, so this class only reads.
 */
class NodeBridge(
    private val source: SnapshotSource,
    private val pollIntervalMillis: Long = 5_000,
) {
    private val state = MutableStateFlow<BridgeState>(
        if (source is HttpSnapshotSource) BridgeState.NotConfigured else BridgeState.Failed(BridgeFailure.Unreachable),
    )

    fun state(): StateFlow<BridgeState> = state.asStateFlow()

    /** Runs one poll synchronously; used at startup and by tests. */
    fun pollOnce() {
        state.value = reduce(source.fetchSnapshot(), ::toBridgeState)
    }

    fun start(scope: CoroutineScope): Job = scope.launch {
        while (currentCoroutineContext().isActive) {
            pollOnce()
            delay(pollIntervalMillis)
        }
    }

    companion object {
        fun toBridgeState(result: SnapshotResult): BridgeState = when (result) {
            is SnapshotResult.Ok -> BridgeState.Ready(result.view.toPageData(), result.view.snapshotRevision)
            is SnapshotResult.Failed -> BridgeState.Failed(result.failure)
        }

        /**
         * Pure reduction step kept separate so the state machine is testable
         * without coroutines or sockets.
         */
        fun reduce(
            result: SnapshotResult,
            map: (SnapshotResult) -> BridgeState,
        ): BridgeState = map(result)

        fun notConfigured(): BridgeState = BridgeState.NotConfigured
    }
}
