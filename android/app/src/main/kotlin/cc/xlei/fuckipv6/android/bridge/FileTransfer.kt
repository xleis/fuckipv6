package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.io.FilterOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

data class TransferRequest(
    val peer: String,
    val shareId: String,
    val path: String,
    val networkId: String? = null,
    val chunkSize: Long? = null,
)

data class StartedTransfer(
    val transferId: String,
    val operationId: String,
    val sizeBytes: Long,
)

data class PendingDownload(
    val token: String,
    val transferId: String,
    val operationId: String,
    val expiresAtUnixMs: Long,
)

data class TransferStatus(
    val transferId: String,
    val operationId: String,
    val direction: String?,
    val state: String?,
    val transferredBytes: Long,
    val totalBytes: Long,
    val receivedChunks: Int,
    val totalChunks: Int,
    val errorCode: String?,
    val failureKind: String?,
    val updatedAtUnixMs: Long?,
) {
    val isActive: Boolean
        get() = state in ACTIVE_STATES

    companion object {
        private val ACTIVE_STATES = setOf("queued", "running", "paused", "waiting_for_retry")
    }
}

sealed interface TransferOutcome<out T> {
    data class Ok<T>(val value: T) : TransferOutcome<T>
    data class Failed(val failure: BridgeFailure) : TransferOutcome<Nothing>
}

object FileTransferJson {
    fun parseStarted(body: String): StartedTransfer {
        val root = JSONObject(body)
        return StartedTransfer(
            transferId = root.stringOrNull("transfer_id")
                ?: throw IllegalArgumentException("transfer_id is required"),
            operationId = root.stringOrNull("operation_id")
                ?: throw IllegalArgumentException("operation_id is required"),
            sizeBytes = root.longOrNull("size_bytes") ?: 0L,
        )
    }

    fun parsePendingDownload(body: String): PendingDownload {
        val root = JSONObject(body)
        return PendingDownload(
            token = root.stringOrNull("token") ?: throw IllegalArgumentException("token is required"),
            transferId = root.stringOrNull("transfer_id")
                ?: throw IllegalArgumentException("transfer_id is required"),
            operationId = root.stringOrNull("operation_id")
                ?: throw IllegalArgumentException("operation_id is required"),
            expiresAtUnixMs = root.longOrNull("expires_at_unix_ms") ?: 0L,
        )
    }

    fun parseStatuses(body: String): List<TransferStatus> {
        val array = try {
            JSONArray(body)
        } catch (_: JSONException) {
            throw IllegalArgumentException("transfer list is not an array")
        }
        return (0 until array.length()).mapNotNull { index ->
            val item = array.optJSONObject(index) ?: return@mapNotNull null
            val transferId = item.stringOrNull("transfer_id") ?: return@mapNotNull null
            val operationId = item.stringOrNull("operation_id") ?: return@mapNotNull null
            TransferStatus(
                transferId = transferId,
                operationId = operationId,
                direction = item.stringOrNull("direction"),
                state = item.stringOrNull("state"),
                transferredBytes = item.longOrNull("transferred_bytes") ?: 0L,
                totalBytes = item.longOrNull("total_bytes") ?: 0L,
                receivedChunks = item.longOrNull("received_chunks")?.toInt() ?: 0,
                totalChunks = item.longOrNull("total_chunks")?.toInt() ?: 0,
                errorCode = item.stringOrNull("error_code"),
                failureKind = item.stringOrNull("failure_kind"),
                updatedAtUnixMs = item.longOrNull("updated_at_unix_ms"),
            )
        }
    }

    fun parseControl(body: String): String {
        val status = JSONObject(body).stringOrNull("status")
            ?: throw IllegalArgumentException("control response has no status")
        return status
    }
}

object FileTransferUrls {
    fun upload(config: NodeApiConfig, request: TransferRequest): String =
        config.baseUrl.trimEnd('/') + UPLOADS_PATH + "?" + query(request, includeChunkSize = true)

    fun downloads(config: NodeApiConfig): String = config.baseUrl.trimEnd('/') + DOWNLOADS_PATH

    fun download(config: NodeApiConfig, token: String): String =
        config.baseUrl.trimEnd('/') + DOWNLOADS_PATH + "/" + URLEncoder.encode(token, "UTF-8")

    fun transfers(config: NodeApiConfig): String = config.baseUrl.trimEnd('/') + TRANSFERS_PATH

    fun control(config: NodeApiConfig, transferId: String, operationId: String, action: String): String {
        require(action in CONTROL_ACTIONS) { "unsupported transfer action: $action" }
        return config.baseUrl.trimEnd('/') + TRANSFERS_PATH + "/" +
            URLEncoder.encode(transferId, "UTF-8") + "/" +
            URLEncoder.encode(operationId, "UTF-8") + "/" + action
    }

    fun downloadRequestBody(request: TransferRequest): String = JSONObject()
        .put("peer", request.peer)
        .put("share_id", request.shareId)
        .put("path", request.path)
        .apply { request.networkId?.let { put("network_id", it) } }
        .apply { request.chunkSize?.let { put("chunk_size", it) } }
        .toString()

    private fun query(request: TransferRequest, includeChunkSize: Boolean): String = buildList {
        add("peer=" + encoder(request.peer))
        request.networkId?.let { add("network_id=" + encoder(it)) }
        add("share_id=" + encoder(request.shareId))
        add("path=" + encoder(request.path))
        if (includeChunkSize) {
            request.chunkSize?.let { add("chunk_size=$it") }
        }
    }.joinToString("&")

    private fun encoder(value: String): String = URLEncoder.encode(value, "UTF-8")

    val CONTROL_ACTIONS = setOf("pause", "resume")
    const val UPLOADS_PATH = "/api/v1/file/uploads"
    const val DOWNLOADS_PATH = "/api/v1/file/downloads"
    const val TRANSFERS_PATH = "/api/v1/file/transfers"
}

/**
 * File transfer initiator/control client.
 *
 * Uploads stream the caller's bytes with a fixed content length; downloads are
 * started by asking for a short-lived token and never carry a path in a query
 * string. Every branch returns a value and nothing is retried silently: an
 * "accepted" transfer is reported as started, never as completed.
 */
class FileTransferClient(
    private val config: NodeApiConfig,
    private val timeoutMillis: Int = 30_000,
) {
    fun startUpload(
        request: TransferRequest,
        source: () -> InputStream,
        contentLength: Long,
    ): TransferOutcome<StartedTransfer> {
        if (contentLength < 0) return TransferOutcome.Failed(BridgeFailure.Misconfigured)
        return exchange(
            url = FileTransferUrls.upload(config, request),
            method = "POST",
            contentType = "application/octet-stream",
            bodyLength = contentLength,
            writeBody = { output -> source().use { it.copyTo(output) } },
            parse = FileTransferJson::parseStarted,
            expected = HttpURLConnection.HTTP_CREATED,
        )
    }

    fun startDownload(request: TransferRequest): TransferOutcome<PendingDownload> =
        exchange(
            url = FileTransferUrls.downloads(config),
            method = "POST",
            contentType = "application/json",
            bodyLength = null,
            writeBody = { output ->
                output.write(FileTransferUrls.downloadRequestBody(request).toByteArray(Charsets.UTF_8))
            },
            parse = FileTransferJson::parsePendingDownload,
            expected = HttpURLConnection.HTTP_CREATED,
        )

    fun listTransfers(): TransferOutcome<List<TransferStatus>> =
        exchange(
            url = FileTransferUrls.transfers(config),
            method = "GET",
            contentType = null,
            bodyLength = null,
            writeBody = null,
            parse = FileTransferJson::parseStatuses,
            expected = HttpURLConnection.HTTP_OK,
        )

    fun pause(transferId: String, operationId: String): TransferOutcome<String> =
        control(transferId, operationId, "pause")

    fun resume(transferId: String, operationId: String): TransferOutcome<String> =
        control(transferId, operationId, "resume")

    private fun control(transferId: String, operationId: String, action: String): TransferOutcome<String> =
        exchange(
            url = FileTransferUrls.control(config, transferId, operationId, action),
            method = "POST",
            contentType = null,
            bodyLength = 0,
            writeBody = null,
            parse = FileTransferJson::parseControl,
            expected = HttpURLConnection.HTTP_OK,
        )

    private fun <T> exchange(
        url: String,
        method: String,
        contentType: String?,
        bodyLength: Long?,
        writeBody: ((java.io.OutputStream) -> Unit)?,
        parse: (String) -> T,
        expected: Int,
    ): TransferOutcome<T> {
        config.validationError()?.let { return TransferOutcome.Failed(BridgeFailure.Misconfigured) }
        val connection = try {
            URL(url).openConnection() as HttpURLConnection
        } catch (_: IOException) {
            return TransferOutcome.Failed(BridgeFailure.Unreachable)
        } catch (_: IllegalArgumentException) {
            return TransferOutcome.Failed(BridgeFailure.Misconfigured)
        }
        return try {
            connection.requestMethod = method
            connection.connectTimeout = timeoutMillis
            connection.readTimeout = timeoutMillis
            connection.instanceFollowRedirects = false
            connection.setRequestProperty("Accept", "application/json")
            config.token?.takeIf { it.isNotBlank() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            contentType?.let { connection.setRequestProperty("Content-Type", it) }
            if (writeBody != null) {
                connection.doOutput = true
                bodyLength?.let { length ->
                    if (length in 1..Int.MAX_VALUE.toLong()) {
                        connection.setFixedLengthStreamingMode(length)
                    } else {
                        connection.setChunkedStreamingMode(64 * 1024)
                    }
                }
                FilterOutputStream(connection.outputStream).use { output ->
                    writeBody(output)
                    output.flush()
                }
            }
            val status = connection.responseCode
            when {
                status == expected -> {
                    val body = connection.inputStream.bufferedReader().use { it.readText() }
                    TransferOutcome.Ok(parse(body))
                }
                status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN ->
                    TransferOutcome.Failed(BridgeFailure.Unauthorized(status))
                else -> TransferOutcome.Failed(BridgeFailure.HttpStatus(status))
            }
        } catch (_: IOException) {
            TransferOutcome.Failed(BridgeFailure.Unreachable)
        } catch (_: JSONException) {
            TransferOutcome.Failed(BridgeFailure.Malformed)
        } catch (_: IllegalArgumentException) {
            TransferOutcome.Failed(BridgeFailure.Malformed)
        } finally {
            connection.disconnect()
        }
    }
}