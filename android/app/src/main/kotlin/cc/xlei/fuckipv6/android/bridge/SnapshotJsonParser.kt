package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.android.ui.FilePageData
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkRole
import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthIssue
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayLifecycleState
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayReachabilityState
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
    /** Parses the File slice only; kept for callers that need nothing else. */
    fun parse(body: String): Result<SnapshotFileView> =
        parseSnapshot(body).map { it.file }

    fun parseSnapshot(body: String): Result<ParsedSnapshot> = runCatching {
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
        ParsedSnapshot(
            file = SnapshotFileView(
                capability = capability,
                networks = networks,
                snapshotRevision = snapshot.optLongOrNull("revision"),
            ),
            relay = relaySurface(snapshot),
        )
    }

    /**
     * Builds the Relay slice. The node only reports a health record once the
     * embedded service exists, and the page treats a missing record as unknown;
     * nothing here guesses a healthy state.
     */
    private fun relaySurface(snapshot: JSONObject): RelaySurface? {
        val network = snapshot.optJSONArray("networks")?.let { array ->
            (0 until array.length()).mapNotNull { index -> array.optJSONObject(index) }
                .firstOrNull { candidate ->
                    val roles = candidate.optJSONArray("roles") ?: return@firstOrNull false
                    (0 until roles.length()).any {
                        wireEnum { NodeNetworkRole.fromWireName(roles.optString(it, "")) } == NodeNetworkRole.RELAY
                    }
                }
        } ?: return null
        val networkName = network.optStringOrNull("name") ?: return null
        val service = snapshot.optJSONArray("embedded_services")?.let { array ->
            (0 until array.length()).mapNotNull { index -> array.optJSONObject(index) }
                .firstOrNull { it.optStringOrNull("network_name") == networkName }
        }
        val permissions = snapshot.optJSONObject("permissions")
            ?.optJSONArray("permissions")
            ?.let { array ->
                (0 until array.length()).mapNotNull { index ->
                    wireEnum { NodePermission.fromWireName(array.optString(index, "")) }
                }.toSet()
            }
            .orEmpty()
        return RelaySurface(
            networkName = networkName,
            relayEnabled = true,
            health = service?.optJSONObject("relay_health")?.let(::relayHealth),
            // Governance events are not rendered yet; an empty projection would
            // claim "no events", so the surface stays null until it is mapped.
            governance = null,
            permissions = permissions,
        )
    }

    private fun relayHealth(json: JSONObject): RelayHealthSnapshot? {
        val status = wireEnum { RelayHealthStatus.fromWireName(json.optStringOrNull("status") ?: return null) }
            ?: return null
        val lifecycle =
            wireEnum { RelayLifecycleState.fromWireName(json.optStringOrNull("lifecycle") ?: return null) }
                ?: return null
        val issues = json.optJSONArray("issues")?.let { array ->
            (0 until array.length()).mapNotNull { index ->
                wireEnum { RelayHealthIssue.fromWireName(array.optString(index, "")) }
            }
        }.orEmpty()
        return RelayHealthSnapshot(
            json.optBoolean("authorization_valid", false),
            json.optBoolean("forwarding_observed", false),
            issues,
            lifecycle,
            null,
            null,
            json.optStringOrNull("listen_addr"),
            json.optBoolean("listener_bound", false),
            wireEnum {
                RelayReachabilityState.fromWireName(
                    json.optStringOrNull("network_reachability") ?: "",
                )
            },
            json.optLongOrNull("observed_at_unix_ms"),
            json.optBoolean("quota_limited", false),
            json.optStringOrNull("reason_code"),
            status,
            null,
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

/**
 * The generated enums throw on unknown wire values. A node newer than this app
 * must degrade to "unknown" for that one field instead of failing the whole
 * snapshot (and never to a value that looks healthy).
 */
private inline fun <T> wireEnum(block: () -> T?): T? = try {
    block()
} catch (_: IllegalArgumentException) {
    null
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
