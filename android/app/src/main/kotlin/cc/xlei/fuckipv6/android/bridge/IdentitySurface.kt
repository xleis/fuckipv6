package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * The node's own identity as the snapshot exposes it.
 *
 * `displayName == null` and `fingerprint == null` mean the viewer is not
 * permitted to see them (the server omits hidden fields); the UI must say
 * "hidden" rather than "unknown", because the node does have both.
 */
data class IdentityView(
    val nodeId: String,
    val displayName: String?,
    val fingerprint: String?,
    val createdAtUnixMs: Long?,
) {
    val displayNameHidden: Boolean
        get() = displayName == null

    val fingerprintHidden: Boolean
        get() = fingerprint == null
}

object IdentitySurface {
    fun parse(snapshot: JSONObject): IdentityView? {
        val identity = snapshot.optJSONObject("identity") ?: return null
        val nodeId = identity.stringOrNull("node_id") ?: return null
        return IdentityView(
            nodeId = nodeId,
            displayName = identity.stringOrNull("display_name"),
            fingerprint = identity.stringOrNull("public_key_fingerprint"),
            createdAtUnixMs = identity.longOrNull("created_at_unix_ms"),
        )
    }
}
