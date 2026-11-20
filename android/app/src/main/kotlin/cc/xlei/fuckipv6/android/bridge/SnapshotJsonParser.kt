package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.android.ui.FilePageData
import org.json.JSONException
import org.json.JSONObject

/** One `file_capability` entry of the node snapshot. */
data class FileCapabilityView(
    val state: String?,
    val networkId: String?,
    val accessEpoch: Long?,
    val shareRevision: Long?,
    val availableBytes: Long?,
    val activeTransfers: Long,
    val rootError: String?,
)

data class NetworkLabel(val networkId: String, val networkName: String)

/** The subset of the snapshot the Android surfaces need today. */
data class SnapshotFileView(
    val capability: FileCapabilityView?,
    val networks: List<NetworkLabel>,
    val snapshotRevision: Long?,
) {
    fun toPageData(): FilePageData = FilePageData.from(capability, networks)
}

/**
 * Parses the versioned `GET /api/v1/snapshot` response.
 *
 * Only the fields the Android surfaces render are read, and nothing is
 * synthesised: a missing `file_capability` stays missing (the page renders
 * `disabled`), and an unreadable body fails closed as `Malformed` instead of
 * producing an empty but "successful" snapshot.
 */
object SnapshotJsonParser {
    fun parse(body: String): Result<SnapshotFileView> = runCatching {
        val envelope = JSONObject(body)
        val snapshot = envelope.optJSONObject("snapshot")
            ?: throw JSONException("snapshot envelope is missing the snapshot object")
        val capability = snapshot.optJSONObject("file_capability")?.let(::capability)
        val networks = snapshot.optJSONArray("memberships")
            ?.let { array ->
                (0 until array.length()).mapNotNull { index ->
                    val membership = array.optJSONObject(index) ?: return@mapNotNull null
                    val id = membership.optString("network_id", "")
                    val name = membership.optString("network_name", "")
                    if (id.isEmpty() || name.isEmpty()) null else NetworkLabel(id, name)
                }
            }
            .orEmpty()
        SnapshotFileView(
            capability = capability,
            networks = networks,
            snapshotRevision = snapshot.optLongOrNull("revision"),
        )
    }

    private fun capability(json: JSONObject) = FileCapabilityView(
        state = json.optStringOrNull("state"),
        networkId = json.optStringOrNull("network_id"),
        accessEpoch = json.optLongOrNull("access_epoch"),
        shareRevision = json.optLongOrNull("share_revision"),
        availableBytes = json.optLongOrNull("available_bytes"),
        activeTransfers = json.optLongOrNull("active_transfers") ?: 0L,
        rootError = json.optStringOrNull("root_error"),
    )
}

private fun JSONObject.optLongOrNull(key: String): Long? {
    if (!has(key) || isNull(key)) return null
    return when (val value = opt(key)) {
        is Number -> value.toLong()
        is String -> value.toLongOrNull()
        else -> null
    }
}

private fun JSONObject.optStringOrNull(key: String): String? {
    if (!has(key) || isNull(key)) return null
    val value = optString(key, "")
    return value.takeIf { it.isNotEmpty() }
}
