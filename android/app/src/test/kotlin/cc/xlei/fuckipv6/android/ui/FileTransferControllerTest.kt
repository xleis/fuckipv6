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
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.InputStream

class FileTransferControllerTest {
    private val running = TransferStatus(
        transferId = "t-1",
        operationId = "op-1",
        direction = "upload",
        state = "running",
        transferredBytes = 10,
        totalBytes = 100,
        receivedChunks = 1,
        totalChunks = 10,
        errorCode = null,
        failureKind = null,
        updatedAtUnixMs = 1,
    )

    private class FakeTransfers : TransferSource {
        var uploadResult: TransferOutcome<StartedTransfer> =
            TransferOutcome.Ok(StartedTransfer("t-1", "op-1", 100))
        var downloadResult: TransferOutcome<PendingDownload> =
            TransferOutcome.Ok(PendingDownload("tok", "t-2", "op-2", 9))
        var listResult: TransferOutcome<List<TransferStatus>> = TransferOutcome.Ok(emptyList())
        var controlResult: TransferOutcome<String> = TransferOutcome.Ok("paused")
        val controls = mutableListOf<String>()
        var uploadCalls = 0

        override fun startUpload(
            request: TransferRequest,
            source: () -> InputStream,
            contentLength: Long,
        ): TransferOutcome<StartedTransfer> {
            uploadCalls += 1
            return uploadResult
        }

        override fun startDownload(request: TransferRequest) = downloadResult
        override fun listTransfers() = listResult
        override fun pause(transferId: String, operationId: String): TransferOutcome<String> {
            controls += "pause:$transferId:$operationId"
            return controlResult
        }
        override fun resume(transferId: String, operationId: String): TransferOutcome<String> {
            controls += "resume:$transferId:$operationId"
            return controlResult
        }
    }

    private fun controller(source: FakeTransfers) =
        FileTransferController(source, CoroutineScope(Dispatchers.Default))

    private fun await(
        controller: FileTransferController,
        predicate: (FileTransferState) -> Boolean,
    ): FileTransferState {
        val deadline = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < deadline) {
            val state = controller.state().value
            if (predicate(state)) return state
            Thread.sleep(5)
        }
        return controller.state().value
    }

    @Test
    fun uploadRecordsAcceptanceAndRefreshesFromTheNode() {
        val source = FakeTransfers()
        source.listResult = TransferOutcome.Ok(listOf(running))
        val controller = controller(source)
        controller.upload(
            TransferRequest("peer-a", "share-1", "docs/a.bin", "net-1"),
            contentLength = 100,
            open = { ByteArrayInputStream(ByteArray(0)) },
        )
        val state = await(controller) { !it.busy && it.transfers.isNotEmpty() }
        assertEquals(1, source.uploadCalls)
        assertEquals("t-1", state.lastStarted?.transferId)
        assertEquals(1, state.activeCount)
        assertNull(state.errorKey)
    }

    @Test
    fun negativeLengthNeverReachesTheSource() {
        val source = FakeTransfers()
        val controller = controller(source)
        controller.upload(
            TransferRequest("p", "s", "a"),
            contentLength = -1,
            open = { ByteArrayInputStream(ByteArray(0)) },
        )
        val state = await(controller) { it.errorKey != null }
        assertEquals("file.page.error.unsupported_detail", state.errorKey)
        assertEquals(0, source.uploadCalls)
    }

    @Test
    fun failuresMapToSurfaceCopyAndKeepTheList() {
        val source = FakeTransfers()
        source.listResult = TransferOutcome.Ok(listOf(running))
        val controller = controller(source)
        controller.refresh()
        await(controller) { it.transfers.isNotEmpty() }

        source.listResult = TransferOutcome.Failed(BridgeFailure.HttpStatus(429))
        controller.refresh()
        val state = await(controller) { it.errorKey != null }
        assertEquals("file.page.error.rate_limited_detail", state.errorKey)
        assertEquals(1, state.transfers.size)
        assertFalse(state.busy)
    }

    @Test
    fun downloadStopsAtTheTokenAndNeverClaimsBytes() {
        val source = FakeTransfers()
        val controller = controller(source)
        controller.download(TransferRequest("p", "s", "docs/a.bin"))
        val state = await(controller) { it.pendingDownload != null }
        assertEquals("tok", state.pendingDownload?.token)
        assertTrue(state.transfers.isEmpty())
    }

    @Test
    fun pauseAndResumeUseTheReportedTransferIdentity() {
        val source = FakeTransfers()
        source.listResult = TransferOutcome.Ok(listOf(running))
        val controller = controller(source)
        controller.pause(running)
        await(controller) { source.controls.isNotEmpty() }
        controller.resume(running)
        await(controller) { source.controls.size == 2 }
        assertEquals(listOf("pause:t-1:op-1", "resume:t-1:op-1"), source.controls)
    }

    @Test
    fun aLocallyDetectedProblemReachesTheSameSurfaceAsANodeError() {
        val controller = FileTransferController(FakeTransfers(), CoroutineScope(Dispatchers.Unconfined))
        controller.reportClientError("file.page.error.invalid_filename_detail")
        assertEquals("file.page.error.invalid_filename_detail", controller.state().value.errorKey)
        assertEquals(false, controller.state().value.busy)
    }
}
