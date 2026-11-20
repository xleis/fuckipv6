package cc.xlei.fuckipv6.android.bridge

import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL

/** One snapshot fetch result. Never throws: failures are values. */
sealed interface SnapshotResult {
    data class Ok(val view: ParsedSnapshot) : SnapshotResult
    data class Failed(val failure: BridgeFailure) : SnapshotResult
}

fun interface SnapshotSource {
    fun fetchSnapshot(): SnapshotResult
}

/**
 * Blocking HTTP source for `GET /api/v1/snapshot`.
 *
 * The node control API is loopback-only by default and authenticates with a
 * bearer token when one is configured, so the request never follows redirects
 * and never retries silently. Every transport problem becomes a [BridgeFailure]
 * the UI can render; nothing is turned into a synthetic snapshot.
 */
class HttpSnapshotSource(
    private val config: NodeApiConfig,
    private val timeoutMillis: Int = 4_000,
) : SnapshotSource {
    override fun fetchSnapshot(): SnapshotResult {
        config.validationError()?.let { return SnapshotResult.Failed(BridgeFailure.Misconfigured) }
        val connection = try {
            URL(config.snapshotUrl).openConnection() as HttpURLConnection
        } catch (_: IOException) {
            return SnapshotResult.Failed(BridgeFailure.Unreachable)
        } catch (_: IllegalArgumentException) {
            return SnapshotResult.Failed(BridgeFailure.Misconfigured)
        }
        return try {
            connection.requestMethod = "GET"
            connection.connectTimeout = timeoutMillis
            connection.readTimeout = timeoutMillis
            connection.instanceFollowRedirects = false
            connection.setRequestProperty("Accept", "application/json")
            config.token?.takeIf { it.isNotBlank() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            val status = connection.responseCode
            when {
                status == HttpURLConnection.HTTP_OK -> {
                    val body = connection.inputStream.bufferedReader().use { it.readText() }
                    SnapshotJsonParser.parseSnapshot(body).fold(
                        onSuccess = { SnapshotResult.Ok(it) },
                        onFailure = { SnapshotResult.Failed(BridgeFailure.Malformed) },
                    )
                }
                status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN ->
                    SnapshotResult.Failed(BridgeFailure.Unauthorized(status))
                else -> SnapshotResult.Failed(BridgeFailure.HttpStatus(status))
            }
        } catch (_: IOException) {
            SnapshotResult.Failed(BridgeFailure.Unreachable)
        } finally {
            connection.disconnect()
        }
    }
}
