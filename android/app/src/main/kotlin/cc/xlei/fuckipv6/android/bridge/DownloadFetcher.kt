package cc.xlei.fuckipv6.android.bridge

import java.io.IOException
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

sealed interface FetchOutcome {
    data class Ok(val bytesWritten: Long) : FetchOutcome
    data class Failed(val failure: BridgeFailure) : FetchOutcome
}

fun interface DownloadFetcher {
    fun fetch(token: String, target: OutputStream, maxBytes: Long): FetchOutcome
}

/**
 * Streams one token-bound download into a caller-provided sink.
 *
 * The sink is the SAF document the operator chose; this class never decides
 * where bytes land. It also refuses to report success after a short read: a
 * broken stream is a failure, and the caller is expected to discard the partial
 * document rather than presenting it as a saved file.
 */
class HttpDownloadFetcher(
    private val config: NodeApiConfig,
    private val timeoutMillis: Int = 60_000,
) : DownloadFetcher {
    override fun fetch(token: String, target: OutputStream, maxBytes: Long): FetchOutcome {
        config.validationError()?.let { return FetchOutcome.Failed(BridgeFailure.Misconfigured) }
        val encoded = URLEncoder.encode(token, "UTF-8")
        val connection = try {
            URL(config.baseUrl.trimEnd('/') + DOWNLOADS_PATH + "/" + encoded)
                .openConnection() as HttpURLConnection
        } catch (_: IOException) {
            return FetchOutcome.Failed(BridgeFailure.Unreachable)
        } catch (_: IllegalArgumentException) {
            return FetchOutcome.Failed(BridgeFailure.Misconfigured)
        }
        return try {
            connection.requestMethod = "GET"
            connection.connectTimeout = timeoutMillis
            connection.readTimeout = timeoutMillis
            connection.instanceFollowRedirects = false
            connection.setRequestProperty("Accept", "application/octet-stream")
            config.token?.takeIf { it.isNotBlank() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            val status = connection.responseCode
            when {
                status == HttpURLConnection.HTTP_OK -> {
                    val declared = connection.getHeaderField("Content-Length")?.toLongOrNull()
                    if (declared != null && declared > maxBytes) {
                        return FetchOutcome.Failed(BridgeFailure.HttpStatus(HttpURLConnection.HTTP_ENTITY_TOO_LARGE))
                    }
                    var written = 0L
                    connection.inputStream.use { input ->
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            val read = input.read(buffer)
                            if (read < 0) break
                            written += read
                            if (written > maxBytes) {
                                // Do not leave an oversize partial document behind.
                                return FetchOutcome.Failed(
                                    BridgeFailure.HttpStatus(HttpURLConnection.HTTP_ENTITY_TOO_LARGE),
                                )
                            }
                            target.write(buffer, 0, read)
                        }
                    }
                    target.flush()
                    if (declared != null && declared != written) {
                        return FetchOutcome.Failed(BridgeFailure.Malformed)
                    }
                    FetchOutcome.Ok(written)
                }
                status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN ->
                    FetchOutcome.Failed(BridgeFailure.Unauthorized(status))
                status == HttpURLConnection.HTTP_NOT_FOUND ->
                    FetchOutcome.Failed(BridgeFailure.HttpStatus(status))
                else -> FetchOutcome.Failed(BridgeFailure.HttpStatus(status))
            }
        } catch (_: IOException) {
            FetchOutcome.Failed(BridgeFailure.Unreachable)
        } finally {
            connection.disconnect()
        }
    }

    companion object {
        /** Same ceiling the node uses for WebUI uploads. */
        const val MAX_BYTES: Long = 2L * 1024 * 1024 * 1024
        private const val DOWNLOADS_PATH = "/api/v1/file/downloads"
    }
}