package cc.xlei.fuckipv6.android.bridge

import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/**
 * Blocking SSE reader for `GET /api/v1/events`.
 *
 * The bridge treats the stream as a *hint*: only a `snapshot` frame is applied
 * directly, everything else just triggers one authoritative snapshot pull. A
 * stream that dies is reported once and left to the caller, which keeps its slow
 * fallback poll — losing the stream must never look like an empty snapshot.
 */
class NodeEventStream(
    private val config: NodeApiConfig,
    private val timeoutMillis: Int = 10_000,
    private val resumeCursor: String? = null,
) {
    /** Reads frames until the stream closes; returns the reason for stopping. */
    fun read(onFrame: (NodeEventFrame) -> Unit): BridgeFailure? {
        config.validationError()?.let { return BridgeFailure.Misconfigured }
        val connection = try {
            URL(eventsUrl(config, resumeCursor)).openConnection() as HttpURLConnection
        } catch (_: IOException) {
            return BridgeFailure.Unreachable
        } catch (_: IllegalArgumentException) {
            return BridgeFailure.Misconfigured
        }
        return try {
            connection.requestMethod = "GET"
            connection.connectTimeout = timeoutMillis
            // A live stream idles between events; a read timeout would tear it
            // down on every quiet period, so only the connect phase is bounded.
            connection.readTimeout = 0
            connection.instanceFollowRedirects = false
            connection.setRequestProperty("Accept", "text/event-stream")
            // The URL path drops a malformed cursor; the header must agree, or the
            // node sees an invalid resume token it would answer 400 to.
            resumeCursor?.trim()
                ?.takeIf { it.isNotEmpty() && isValidCursor(it) }
                ?.let { connection.setRequestProperty("Last-Event-ID", it) }
            config.token?.takeIf { it.isNotBlank() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            val status = connection.responseCode
            if (status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN) {
                return BridgeFailure.Unauthorized(status)
            }
            if (status != HttpURLConnection.HTTP_OK) {
                return BridgeFailure.HttpStatus(status)
            }
            connection.inputStream.bufferedReader().useLines { lines ->
                for (line in lines) {
                    EventFrameParser.parse(line)?.let(onFrame)
                }
            }
            null
        } catch (_: IOException) {
            BridgeFailure.Unreachable
        } finally {
            connection.disconnect()
        }
    }

    companion object {
        const val EVENTS_PATH = "/api/v1/events"

        /**
         * Resume URL. The node accepts the same token as `?after=` and as the
         * `Last-Event-ID` header; sending both keeps reconnects consistent with
         * SSE proxies that drop one of them. A malformed token is dropped rather
         * than sent: the node would answer 400 and the stream would never start.
         */
        fun eventsUrl(config: NodeApiConfig, resumeCursor: String?): String {
            val base = config.baseUrl.trimEnd('/') + EVENTS_PATH
            val token = resumeCursor?.trim()?.takeIf { it.isNotEmpty() && isValidCursor(it) }
                ?: return base
            return base + "?after=" + java.net.URLEncoder.encode(token, "UTF-8")
        }

        fun isValidCursor(token: String): Boolean {
            val parts = token.split(':')
            if (parts.size > 2) return false
            return parts.all { part -> part.isNotEmpty() && part.all(Char::isDigit) }
        }
    }
}