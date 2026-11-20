package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FileTransferTest {
    private val config = NodeApiConfig("http://127.0.0.1:6668")

    @Test
    fun uploadUrlEncodesTheQueryAndNeverPutsBytesInIt() {
        val url = FileTransferUrls.upload(
            config,
            TransferRequest(
                peer = "peer a",
                shareId = "share/1",
                path = "文档/报表.md",
                networkId = "net-1",
                chunkSize = 262_144,
            ),
        )
        assertTrue(url.startsWith("http://127.0.0.1:6668/api/v1/file/uploads?"))
        assertTrue(url.contains("peer=peer+a"))
        assertTrue(url.contains("share_id=share%2F1"))
        assertTrue(url.contains("network_id=net-1"))
        assertTrue(url.contains("path=%E6%96%87%E6%A1%A3%2F%E6%8A%A5%E8%A1%A8.md"))
        assertTrue(url.contains("chunk_size=262144"))

        val minimal = FileTransferUrls.upload(config, TransferRequest(peer = "p", shareId = "s", path = ""))
        assertFalse(minimal.contains("network_id="))
        assertFalse(minimal.contains("chunk_size="))
    }

    @Test
    fun downloadRequestKeepsThePathInTheBodyNotTheQuery() {
        val body = FileTransferUrls.downloadRequestBody(
            TransferRequest(peer = "p", shareId = "s", path = "docs/a b.md", networkId = "net-1"),
        )
        assertTrue(body.contains(""""path":"docs/a b.md""""))
        assertTrue(body.contains(""""network_id":"net-1""""))
        assertFalse(body.contains("chunk_size"))

        val downloads = FileTransferUrls.downloads(config)
        assertEquals("http://127.0.0.1:6668/api/v1/file/downloads", downloads)
        assertEquals(
            "http://127.0.0.1:6668/api/v1/file/downloads/token%2F1",
            FileTransferUrls.download(config, "token/1"),
        )
    }

    @Test
    fun controlUrlsAreTypedAndRejectUnknownActions() {
        assertEquals(
            "http://127.0.0.1:6668/api/v1/file/transfers/t-1/op-1/pause",
            FileTransferUrls.control(config, "t-1", "op-1", "pause"),
        )
        assertEquals(
            "http://127.0.0.1:6668/api/v1/file/transfers/t-1/op-1/resume",
            FileTransferUrls.control(config, "t-1", "op-1", "resume"),
        )
        assertTrue(runCatching { FileTransferUrls.control(config, "t", "o", "delete") }.isFailure)
        assertEquals(
            "http://127.0.0.1:6668/api/v1/file/transfers",
            FileTransferUrls.transfers(config),
        )
    }

    @Test
    fun parsesStartedDownloadAndStatusPayloads() {
        val started = FileTransferJson.parseStarted(
            """{"transfer_id":"t-1","operation_id":"op-1","size_bytes":4096}""",
        )
        assertEquals("t-1", started.transferId)
        assertEquals(4096L, started.sizeBytes)

        val pending = FileTransferJson.parsePendingDownload(
            """{"token":"tok","transfer_id":"t-2","operation_id":"op-2","expires_at_unix_ms":1760000000000}""",
        )
        assertEquals("tok", pending.token)
        assertEquals(1760000000000L, pending.expiresAtUnixMs)

        val statuses = FileTransferJson.parseStatuses(
            """
            [
              {"transfer_id":"t-1","operation_id":"op-1","direction":"upload","state":"running",
               "transferred_bytes":2048,"total_bytes":4096,"received_chunks":1,"total_chunks":2,
               "updated_at_unix_ms":1760000000000},
              {"transfer_id":"t-2","operation_id":"op-2","direction":"download","state":"failed",
               "transferred_bytes":0,"total_bytes":10,"received_chunks":0,"total_chunks":1,
               "error_code":"file.error.rate_limited","failure_kind":"retryable"}
            ]
            """.trimIndent(),
        )
        assertEquals(2, statuses.size)
        assertTrue(statuses[0].isActive)
        assertFalse(statuses[1].isActive)
        assertEquals("file.error.rate_limited", statuses[1].errorCode)

        assertEquals("paused", FileTransferJson.parseControl("""{"status":"paused"}"""))
        assertEquals("resumed", FileTransferJson.parseControl("""{"status":"resumed"}"""))
    }

    @Test
    fun malformedTransferPayloadsAreRejected() {
        assertTrue(runCatching { FileTransferJson.parseStarted("{}") }.isFailure)
        assertTrue(runCatching { FileTransferJson.parsePendingDownload("not json") }.isFailure)
        assertTrue(runCatching { FileTransferJson.parseStatuses("""{"transfer_id":"t"}""") }.isFailure)
        assertTrue(runCatching { FileTransferJson.parseControl("{}") }.isFailure)
    }

    @Test
    fun misconfiguredClientFailsBeforeAnyTransfer() {
        val client = FileTransferClient(NodeApiConfig("localhost"))
        assertTrue(client.startDownload(TransferRequest("p", "s", "a")).let {
            it is TransferOutcome.Failed && it.failure == BridgeFailure.Misconfigured
        })
        assertTrue(client.listTransfers().let {
            it is TransferOutcome.Failed && it.failure == BridgeFailure.Misconfigured
        })
        assertTrue(client.startUpload(TransferRequest("p", "s", "a"), { ByteArrayInputStream(ByteArray(0)) }, 0).let {
            it is TransferOutcome.Failed && it.failure == BridgeFailure.Misconfigured
        })
        assertTrue(client.startUpload(TransferRequest("p", "s", "a"), { ByteArrayInputStream(ByteArray(0)) }, -1).let {
            it is TransferOutcome.Failed
        })
        assertNull(FileTransferUrls.downloadRequestBody(TransferRequest("p", "s", "a")).let { null })
    }

    private class ByteArrayInputStream(private val bytes: ByteArray) : java.io.InputStream() {
        private var index = 0
        override fun read(): Int =
            if (index >= bytes.size) -1 else bytes[index++].toInt() and 0xFF
    }
}
