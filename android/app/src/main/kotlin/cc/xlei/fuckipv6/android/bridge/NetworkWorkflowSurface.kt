package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/**
 * Authoritative network lifecycle, creation, discovery, join and coordinator
 * candidate command builders (W16-05).
 *
 * Every command literal here matches `contracts/typescript/fixtures/node-manifest.json`
 * and is verified automatically by `scripts/android-capability-matrix.py`.
 */
object NetworkWorkflowSurface {
    fun createNetworkCommand(
        name: String,
        purpose: String = "Android mobile mesh network",
        visibility: String = "unlisted",
        joinMode: String = "invite_only",
        signalingPort: Int = 0,
        relayPort: Int = 0,
    ): JSONObject? {
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return null
        val payload = JSONObject()
            .put("name", trimmed)
            .put("purpose", purpose.ifBlank { trimmed })
            .put("visibility", visibility)
            .put("join_mode", joinMode)
        if (signalingPort > 0) payload.put("signaling_port", signalingPort)
        if (relayPort > 0) payload.put("relay_port", relayPort)
        return JSONObject()
            .put("command", "create_network")
            .put("payload", payload)
    }

    fun joinNetworkCommand(
        ticket: String,
        name: String? = null,
        exitContributionAcknowledged: Boolean = false,
    ): JSONObject? {
        val trimmed = ticket.trim()
        if (trimmed.isEmpty()) return null
        val payload = JSONObject()
            .put("ticket", trimmed)
            .put("exit_contribution_acknowledged", exitContributionAcknowledged)
        name?.trim()?.takeIf { it.isNotEmpty() }?.let { payload.put("name", it) }
        return JSONObject()
            .put("command", "join_network")
            .put("payload", payload)
    }

    fun previewJoinNetworkCommand(networkId: String): JSONObject? {
        val trimmed = networkId.trim()
        if (trimmed.isEmpty()) return null
        return JSONObject()
            .put("command", "preview_join_network")
            .put("payload", JSONObject().put("network_id", trimmed))
    }

    fun previewInvitationCommand(
        networkId: String,
        invitationCode: String,
        coordinatorUrl: String? = null,
    ): JSONObject? {
        val nid = networkId.trim()
        val code = invitationCode.trim()
        if (nid.isEmpty() || code.isEmpty()) return null
        val payload = JSONObject()
            .put("network_id", nid)
            .put("invitation_code", code)
        coordinatorUrl?.trim()?.takeIf { it.isNotEmpty() }?.let { payload.put("coordinator_url", it) }
        return JSONObject()
            .put("command", "preview_invitation")
            .put("payload", payload)
    }

    fun redeemInvitationCommand(
        networkId: String,
        invitationCode: String,
        termsAccepted: Boolean = true,
        exitContributionAcknowledged: Boolean = false,
    ): JSONObject? {
        val nid = networkId.trim()
        val code = invitationCode.trim()
        if (nid.isEmpty() || code.isEmpty()) return null
        return JSONObject()
            .put("command", "redeem_invitation")
            .put(
                "payload",
                JSONObject()
                    .put("network_id", nid)
                    .put("invitation_code", code)
                    .put("terms_accepted", termsAccepted)
                    .put("exit_contribution_acknowledged", exitContributionAcknowledged),
            )
    }

    fun refreshDiscoveryIndexCommand(query: String? = null): JSONObject {
        val payload = JSONObject()
        query?.trim()?.takeIf { it.isNotEmpty() }?.let { payload.put("query", it) }
        return JSONObject()
            .put("command", "refresh_discovery_index")
            .put("payload", payload)
    }

    fun retryDiscoveryIndexCommand(): JSONObject = JSONObject()
        .put("command", "retry_discovery_index")

    fun startNodeCommand(): JSONObject = JSONObject().put("command", "start_node")

    fun stopNodeCommand(): JSONObject = JSONObject().put("command", "stop_node")

    fun startNetworksCommand(networks: List<String>): JSONObject = JSONObject()
        .put("command", "start_networks")
        .put("payload", JSONObject().put("networks", JSONArray(networks)))

    fun stopNetworksCommand(networks: List<String>): JSONObject = JSONObject()
        .put("command", "stop_networks")
        .put("payload", JSONObject().put("networks", JSONArray(networks)))

    fun restartNetworksCommand(networks: List<String>): JSONObject = JSONObject()
        .put("command", "restart_networks")
        .put("payload", JSONObject().put("networks", JSONArray(networks)))

    fun rebuildTunCommand(networkName: String): JSONObject? {
        val trimmed = networkName.trim()
        if (trimmed.isEmpty()) return null
        return JSONObject()
            .put("command", "rebuild_tun")
            .put("payload", JSONObject().put("network_name", trimmed))
    }

    fun updateCoordinatorCandidatesCommand(
        networkName: String,
        enabled: Boolean,
        expectedRevision: Long,
    ): JSONObject? {
        val trimmed = networkName.trim()
        if (trimmed.isEmpty() || expectedRevision < 0L) return null
        return JSONObject()
            .put("command", "update_coordinator_candidates")
            .put(
                "payload",
                JSONObject()
                    .put("network_name", trimmed)
                    .put("local_candidate_enabled", enabled)
                    .put("expected_revision", expectedRevision),
            )
    }
}
