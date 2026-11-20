package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/** One peer that exposes a File share. */
data class FileTarget(
    val peer: String,
    val networkId: String,
    val networkName: String,
    val shareId: String,
)

/** One directory entry. Paths are portable shared paths, never host paths. */
data class DirectoryEntry(
    val path: String,
    val isDirectory: Boolean,
    val size: Long,
    val modifiedAtUnixMs: Long?,
)

/**
 * One bounded directory page. `nextCursor` is opaque and only valid for the same
 * directory generation, so it is passed back verbatim.
 */
data class DirectoryPage(
    val entries: List<DirectoryEntry>,
    val nextCursor: String?,
    val truncated: Boolean,
)

data class BrowseRequest(
    val peer: String,
    val shareId: String,
    val networkId: String? = null,
    val path: String = "",
    val limit: Int = 100,
    val cursor: String? = null,
)

sealed interface BrowseResult<out T> {
    data class Ok<T>(val value: T) : BrowseResult<T>
    data class Failed(val failure: BridgeFailure) : BrowseResult<Nothing>
}

object FileBrowseJson {
    fun parseTargets(body: String): List<FileTarget> {
        val array = try {
            JSONArray(body)
        } catch (_: JSONException) {
            throw IllegalArgumentException("targets payload is not an array")
        }
        return (0 until array.length()).mapNotNull { index ->
            val item = array.optJSONObject(index) ?: return@mapNotNull null
            val peer = item.stringOrNull("peer") ?: return@mapNotNull null
            val shareId = item.stringOrNull("share_id") ?: return@mapNotNull null
            val networkId = item.stringOrNull("network_id") ?: return@mapNotNull null
            FileTarget(
                peer = peer,
                networkId = networkId,
                networkName = item.stringOrNull("network_name") ?: networkId,
                shareId = shareId,
            )
        }
    }

    fun parseDirectory(body: String): DirectoryPage {
        val root = JSONObject(body)
        val rawEntries = root.opt("entries")
        val entriesArray = when (rawEntries) {
            null, JSONObject.NULL -> null
            // A wrong-typed field is malformed, not an empty directory: showing
            // "no files" for a broken payload would hide the failure.
            is JSONArray -> rawEntries
            else -> throw IllegalArgumentException("entries must be an array")
        }
        val entries = entriesArray?.let { array ->
            (0 until array.length()).mapNotNull { index ->
                val entry = array.optJSONObject(index) ?: return@mapNotNull null
                val path = entry.stringOrNull("path") ?: return@mapNotNull null
                DirectoryEntry(
                    path = path,
                    isDirectory = entry.optBoolean("is_directory", false),
                    size = entry.longOrNull("size") ?: 0L,
                    modifiedAtUnixMs = entry.longOrNull("modified_at_unix_ms"),
                )
            }
        }.orEmpty()
        return DirectoryPage(
            entries = entries,
            nextCursor = root.stringOrNull("next_cursor"),
            truncated = root.optBoolean("truncated", false),
        )
    }
}

object FileBrowseUrls {
    fun targets(config: NodeApiConfig): String = config.baseUrl.trimEnd('/') + TARGETS_PATH

    fun directory(config: NodeApiConfig, request: BrowseRequest): String {
        val query = buildList {
            add("peer=" + encode(request.peer))
            add("share_id=" + encode(request.shareId))
            request.networkId?.let { add("network_id=" + encode(it)) }
            add("path=" + encode(request.path))
            add("limit=" + request.limit)
            request.cursor?.let { add("cursor=" + encode(it)) }
        }.joinToString("&")
        return config.baseUrl.trimEnd('/') + DIRECTORY_PATH + "?" + query
    }

    private fun encode(value: String): String = URLEncoder.encode(value, "UTF-8")

    const val TARGETS_PATH = "/api/v1/file/targets"
    const val DIRECTORY_PATH = "/api/v1/file/directory"
}

/**
 * Read-only File browser for the Android surface.
 *
 * Browsing only: uploads and downloads stay on the desktop surfaces until the
 * Android app has a transfer surface. Every failure is a value the UI renders;
 * nothing is retried silently and no listing is invented.
 */
class FileBrowseClient(
    private val config: NodeApiConfig,
    private val timeoutMillis: Int = 6_000,
) {
    fun targets(): BrowseResult<List<FileTarget>> = request(FileBrowseUrls.targets(config)) { body ->
        BrowseResult.Ok(FileBrowseJson.parseTargets(body))
    }

    fun directory(request: BrowseRequest): BrowseResult<DirectoryPage> =
        request(FileBrowseUrls.directory(config, request)) { body ->
            BrowseResult.Ok(FileBrowseJson.parseDirectory(body))
        }

    private fun <T> request(url: String, parse: (String) -> BrowseResult<T>): BrowseResult<T> {
        config.validationError()?.let { return BrowseResult.Failed(BridgeFailure.Misconfigured) }
        val connection = try {
            URL(url).openConnection() as HttpURLConnection
        } catch (_: IOException) {
            return BrowseResult.Failed(BridgeFailure.Unreachable)
        } catch (_: IllegalArgumentException) {
            return BrowseResult.Failed(BridgeFailure.Misconfigured)
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
                    try {
                        parse(body)
                    } catch (_: JSONException) {
                        BrowseResult.Failed(BridgeFailure.Malformed)
                    } catch (_: IllegalArgumentException) {
                        BrowseResult.Failed(BridgeFailure.Malformed)
                    }
                }
                status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN ->
                    BrowseResult.Failed(BridgeFailure.Unauthorized(status))
                else -> BrowseResult.Failed(BridgeFailure.HttpStatus(status))
            }
        } catch (_: IOException) {
            BrowseResult.Failed(BridgeFailure.Unreachable)
        } finally {
            connection.disconnect()
        }
    }
}