package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.PendingDownload
import cc.xlei.fuckipv6.android.bridge.StartedTransfer
import cc.xlei.fuckipv6.android.bridge.TransferOutcome
import cc.xlei.fuckipv6.android.bridge.TransferRequest
import cc.xlei.fuckipv6.android.bridge.TransferSource
import cc.xlei.fuckipv6.android.bridge.TransferStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.InputStream

/** Transfer list state. Progress is what the node reports, never a guess. */
data class FileTransferState(
    val transfers: List<TransferStatus> = emptyList(),
    val busy: Boolean = false,
    val errorKey: String? = null,
    val lastStarted: StartedTransfer? = null,
    val pendingDownload: PendingDownload? = null,
) {
    val activeCount: Int
        get() = transfers.count { it.isActive }
}

/**
 * Drives transfers through [TransferSource].
 *
 * Starting a transfer only records that the node accepted it: the list is then
 * refreshed from the node, so "uploading" never becomes "done" locally. Saving a
 * downloaded stream needs a storage provider (SAF), which is why the controller
 * stops at the token.
 */
class FileTransferController(
    private val source: TransferSource,
    private val scope: CoroutineScope,
) {
    private val state = MutableStateFlow(FileTransferState())

    fun state(): StateFlow<FileTransferState> = state.asStateFlow()

    fun refresh() {
        scope.launch {
            val result = withContext(Dispatchers.IO) { source.listTransfers() }
            state.value = when (result) {
                is TransferOutcome.Ok -> state.value.copy(
                    transfers = result.value,
                    busy = false,
                    errorKey = null,
                )
                is TransferOutcome.Failed -> state.value.copy(
                    busy = false,
                    errorKey = result.failure.transferErrorKey(),
                )
            }
        }
    }

    fun upload(
        request: TransferRequest,
        contentLength: Long,
        open: () -> InputStream,
    ) {
        if (contentLength < 0) {
            state.value = state.value.copy(errorKey = "file.page.error.missing_filename")
            return
        }
        state.value = state.value.copy(busy = true, errorKey = null)
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                source.startUpload(request, open, contentLength)
            }
            state.value = when (result) {
                is TransferOutcome.Ok -> state.value.copy(
                    busy = false,
                    lastStarted = result.value,
                    errorKey = null,
                )
                is TransferOutcome.Failed -> state.value.copy(
                    busy = false,
                    errorKey = result.failure.transferErrorKey(),
                )
            }
            refresh()
        }
    }

    fun download(request: TransferRequest) {
        state.value = state.value.copy(busy = true, errorKey = null)
        scope.launch {
            val result = withContext(Dispatchers.IO) { source.startDownload(request) }
            state.value = when (result) {
                is TransferOutcome.Ok -> state.value.copy(
                    busy = false,
                    pendingDownload = result.value,
                    errorKey = null,
                )
                is TransferOutcome.Failed -> state.value.copy(
                    busy = false,
                    errorKey = result.failure.transferErrorKey(),
                )
            }
            refresh()
        }
    }

    fun pause(status: TransferStatus) {
        control(status, source::pause)
    }

    fun resume(status: TransferStatus) {
        control(status, source::resume)
    }

    private fun control(
        status: TransferStatus,
        action: (String, String) -> TransferOutcome<String>,
    ) {
        state.value = state.value.copy(busy = true, errorKey = null)
        scope.launch {
            val result = withContext(Dispatchers.IO) { action(status.transferId, status.operationId) }
            state.value = when (result) {
                is TransferOutcome.Ok -> state.value.copy(busy = false, errorKey = null)
                is TransferOutcome.Failed -> state.value.copy(
                    busy = false,
                    errorKey = result.failure.transferErrorKey(),
                )
            }
            refresh()
        }
    }
}

/** Transfer failures reuse the shared File error copy. */
fun BridgeFailure.transferErrorKey(): String = when (this) {
    is BridgeFailure.Unauthorized -> "file.page.error.permission_detail"
    BridgeFailure.Misconfigured -> "file.page.error.unsupported_detail"
    BridgeFailure.Malformed -> "file.page.error.generic"
    BridgeFailure.Unreachable -> "file.page.error.unavailable_detail"
    is BridgeFailure.HttpStatus ->
        if (status == 429) "file.page.error.rate_limited_detail" else "file.page.error.unavailable_detail"
}
