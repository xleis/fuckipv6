package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * This node's own Exit capability.
 *
 * `network_allowed` is the network administrator's decision and
 * `owner_exit_enabled` is the local owner's; both must be true for the node to
 * serve Exit traffic. The surface shows them separately so an operator can see
 * which side is blocking instead of guessing.
 */
data class LocalExitView(
    val ownerExitEnabled: Boolean,
    val networkAllowed: Boolean,
    val stage: String?,
    val healthy: Boolean,
    val activeClients: Int,
    val totalClients: Long,
    val problemCode: String?,
) {
    val serving: Boolean
        get() = ownerExitEnabled && networkAllowed && healthy && stage == "enabled"
}

/** One network's Exit selection for this node. */
data class ExitSelectionView(
    val networkId: String,
    val policyRevision: Long,
    val mode: String?,
    val fixedExit: String?,
    val selectedDefault: String?,
    val selectableCount: Int,
    val candidateCount: Int,
    val evaluatedAtUnixMs: Long?,
)

object ExitSurface {
    /**
     * Owner-controlled local Exit intent. The command only carries the flag; the
     * node still enforces its own capability gate, so a phone cannot make an
     * unsupported host serve Exit traffic.
     */
    fun localExitCommand(enabled: Boolean): JSONObject = JSONObject()
        .put("command", "set_local_exit_capability")
        .put("payload", JSONObject().put("enabled", enabled))

    fun parseLocalExit(snapshot: JSONObject): LocalExitView? {
        val local = snapshot.optJSONObject("local_exit") ?: return null
        return parseLocalExitObject(local)
    }

    /** Parses the bare `local_exit_capability` projection returned by the command. */
    fun parseLocalExitObject(local: JSONObject): LocalExitView =
        LocalExitView(
            ownerExitEnabled = local.optBoolean("owner_exit_enabled", false),
            networkAllowed = local.optBoolean("network_allowed", false),
            stage = local.stringOrNull("stage"),
            healthy = local.optBoolean("healthy", false),
            activeClients = local.longOrNull("active_clients")?.toInt() ?: 0,
            totalClients = local.longOrNull("total_clients") ?: 0L,
            problemCode = local.optJSONObject("problem")?.stringOrNull("code"),
        )

    fun parseSelections(snapshot: JSONObject): List<ExitSelectionView>? {
        val raw = snapshot.opt("exit_selections") ?: return null
        val array = when (raw) {
            is org.json.JSONArray -> raw
            JSONObject.NULL -> org.json.JSONArray()
            else -> return null
        }
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(::selection)
        }
    }

    private fun selection(json: JSONObject): ExitSelectionView? {
        val networkId = json.stringOrNull("network_id") ?: return null
        val resolution = json.optJSONObject("resolution")
        return ExitSelectionView(
            networkId = networkId,
            policyRevision = json.longOrNull("policy_revision") ?: 0L,
            mode = resolution?.stringOrNull("mode"),
            fixedExit = resolution?.stringOrNull("fixed_exit"),
            selectedDefault = resolution?.stringOrNull("selected_default"),
            selectableCount = resolution?.optJSONArray("selectable_exits")?.length() ?: 0,
            candidateCount = resolution?.optJSONArray("candidates")?.length() ?: 0,
            evaluatedAtUnixMs = json.longOrNull("evaluated_at_unix_ms"),
        )
    }
}
