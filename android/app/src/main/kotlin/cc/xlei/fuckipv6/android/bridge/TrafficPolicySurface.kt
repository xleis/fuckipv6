package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/**
 * Authoritative Traffic Policy command & projection surface (W11-08 / W16-05).
 *
 * Mounts all 7 shared traffic-policy commands from `node-manifest.json` so
 * Android, WebUI and GPUI share the exact same policy draft, preview, test,
 * apply, history and rollback semantics.
 */
data class TrafficRuleItem(
    val id: String,
    val name: String,
    val enabled: Boolean,
    val matcherKind: String,
    val matcherValue: String,
    val actionKind: String,
    val actionTarget: String?,
    val fixedExit: Boolean = false,
)

data class TrafficPolicyDocument(
    val revision: Long,
    val defaultActionKind: String,
    val defaultActionTarget: String?,
    val rules: List<TrafficRuleItem>,
    val rawPolicy: JSONObject,
)

data class TrafficPolicyPreviewView(
    val currentRevision: Long,
    val candidateRevision: Long,
    val riskSummary: String?,
    val changeCount: Int,
    val requiresRiskConfirm: Boolean,
)

data class TrafficPolicyTestView(
    val matchedRuleId: String?,
    val currentAction: String,
    val candidateAction: String,
    val changed: Boolean,
)

data class TrafficPolicyRevisionView(
    val revision: Long,
    val action: String,
    val timestampUnixMs: Long?,
)

object TrafficPolicySurface {
    val ACTION_KINDS = listOf("direct", "reject", "network", "exit")
    val MATCHER_KINDS = listOf("domain_suffix", "domain_exact", "ip_cidr", "port_range")

    fun getPolicyCommand(): JSONObject = JSONObject()
        .put("command", "get_traffic_policy")

    fun getHistoryCommand(): JSONObject = JSONObject()
        .put("command", "get_traffic_policy_history")

    fun previewCommand(expectedRevision: Long, draft: JSONObject): JSONObject? {
        if (expectedRevision < 0L) return null
        return JSONObject()
            .put("command", "preview_traffic_policy")
            .put(
                "payload",
                JSONObject()
                    .put("expected_revision", expectedRevision)
                    .put("draft", draft),
            )
    }

    fun applyCommand(
        expectedRevision: Long,
        draft: JSONObject,
        confirmRisk: Boolean,
    ): JSONObject? {
        if (expectedRevision < 0L) return null
        return JSONObject()
            .put("command", "apply_traffic_policy")
            .put(
                "payload",
                JSONObject()
                    .put("expected_revision", expectedRevision)
                    .put("draft", draft)
                    .put("confirm_risk", confirmRisk),
            )
    }

    fun previewRollbackCommand(expectedRevision: Long, targetRevision: Long): JSONObject? {
        if (expectedRevision < 0L || targetRevision <= 0L) return null
        return JSONObject()
            .put("command", "preview_traffic_policy_rollback")
            .put(
                "payload",
                JSONObject()
                    .put("expected_revision", expectedRevision)
                    .put("target_revision", targetRevision),
            )
    }

    fun rollbackCommand(
        expectedRevision: Long,
        targetRevision: Long,
        confirmRisk: Boolean,
    ): JSONObject? {
        if (expectedRevision < 0L || targetRevision <= 0L) return null
        return JSONObject()
            .put("command", "rollback_traffic_policy")
            .put(
                "payload",
                JSONObject()
                    .put("expected_revision", expectedRevision)
                    .put("target_revision", targetRevision)
                    .put("confirm_risk", confirmRisk),
            )
    }

    fun testCommand(
        current: JSONObject,
        candidate: JSONObject,
        destination: String,
        protocol: String = "tcp",
        port: Int = 443,
    ): JSONObject? {
        val target = destination.trim()
        if (target.isEmpty() || port !in 1..65535) return null
        val input = JSONObject()
            .put("protocol", protocol)
            .put("destination_port", port)
        if (looksLikeIp(target)) {
            input.put("destination_ip", target)
        } else {
            input.put("domain", target)
        }
        return JSONObject()
            .put("command", "test_traffic_policy")
            .put(
                "payload",
                JSONObject()
                    .put("current", current)
                    .put("candidate", candidate)
                    .put("input", input),
            )
    }

    fun buildDraftJson(
        defaultActionKind: String,
        defaultActionTarget: String?,
        rules: List<TrafficRuleItem>,
    ): JSONObject {
        val defaultAction = encodeAction(defaultActionKind, defaultActionTarget)
        val rulesArray = JSONArray()
        rules.forEachIndexed { index, rule ->
            val matcher = when (rule.matcherKind) {
                "domain_exact" -> JSONObject().put("domain_exact", rule.matcherValue.trim())
                "ip_cidr" -> JSONObject().put("cidr", rule.matcherValue.trim())
                "port_range" -> JSONObject().put("port", rule.matcherValue.trim())
                else -> JSONObject().put("domain_suffix", rule.matcherValue.trim())
            }
            val item = JSONObject()
                .put("id", rule.id)
                .put("name", rule.name.ifBlank { rule.id })
                .put("enabled", rule.enabled)
                .put("priority", index + 1)
                .put("matcher", matcher)
                .put("action", encodeAction(rule.actionKind, rule.actionTarget))
                .put("fixed_exit", rule.fixedExit)
            rulesArray.put(item)
        }
        return JSONObject()
            .put("default_action", defaultAction)
            .put("rules", rulesArray)
    }

    fun parseDocument(data: JSONObject?): TrafficPolicyDocument? {
        if (data == null) return null
        val policy = data.optJSONObject("policy") ?: data
        val revision = policy.longOrNull("revision") ?: data.longOrNull("revision") ?: 0L
        val defaultAction = policy.optJSONObject("default_action")
        val defaultKind = defaultAction?.stringOrNull("kind") ?: defaultAction?.keys()?.let { keys ->
            if (keys.hasNext()) keys.next() else null
        } ?: "direct"
        val defaultTarget = defaultAction?.stringOrNull("value")
            ?: defaultAction?.optString(defaultKind)?.takeIf { it.isNotBlank() && it != "null" }
        val rulesJson = policy.optJSONArray("rules") ?: JSONArray()
        val rules = (0 until rulesJson.length()).mapNotNull { idx ->
            val obj = rulesJson.optJSONObject(idx) ?: return@mapNotNull null
            val id = obj.stringOrNull("id") ?: "rule-$idx"
            val name = obj.stringOrNull("name") ?: id
            val enabled = obj.optBoolean("enabled", true)
            val matcher = obj.optJSONObject("matcher")
            val matcherKey = matcher?.keys()?.let { if (it.hasNext()) it.next() else null } ?: "domain_suffix"
            val matcherVal = matcher?.optString(matcherKey, "").orEmpty()
            val action = obj.optJSONObject("action")
            val actionKind = action?.stringOrNull("kind") ?: "direct"
            val actionTarget = action?.stringOrNull("value")
            TrafficRuleItem(
                id = id,
                name = name,
                enabled = enabled,
                matcherKind = matcherKey,
                matcherValue = matcherVal,
                actionKind = actionKind,
                actionTarget = actionTarget,
                fixedExit = obj.optBoolean("fixed_exit", false),
            )
        }
        return TrafficPolicyDocument(
            revision = revision,
            defaultActionKind = defaultKind,
            defaultActionTarget = defaultTarget,
            rules = rules,
            rawPolicy = policy,
        )
    }

    fun parsePreview(data: JSONObject?): TrafficPolicyPreviewView? {
        if (data == null) return null
        val risks = data.optJSONArray("risks")
        val diff = data.optJSONArray("changes") ?: data.optJSONArray("diff")
        val riskSummary = if (risks != null && risks.length() > 0) {
            (0 until risks.length()).mapNotNull { i ->
                risks.optJSONObject(i)?.stringOrNull("kind") ?: risks.optString(i).takeIf { it.isNotBlank() }
            }.joinToString(", ")
        } else {
            null
        }
        return TrafficPolicyPreviewView(
            currentRevision = data.longOrNull("current_revision") ?: 0L,
            candidateRevision = data.longOrNull("candidate_revision") ?: 0L,
            riskSummary = riskSummary,
            changeCount = diff?.length() ?: 0,
            requiresRiskConfirm = (risks?.length() ?: 0) > 0,
        )
    }

    fun parseTest(data: JSONObject?): TrafficPolicyTestView? {
        if (data == null) return null
        val current = data.optJSONObject("current")
        val candidate = data.optJSONObject("candidate")
        val curAction = current?.optJSONObject("action")?.stringOrNull("kind")
            ?: current?.stringOrNull("action")
            ?: "unknown"
        val candAction = candidate?.optJSONObject("action")?.stringOrNull("kind")
            ?: candidate?.stringOrNull("action")
            ?: curAction
        val matched = candidate?.stringOrNull("matched_rule_id")
            ?: current?.stringOrNull("matched_rule_id")
        return TrafficPolicyTestView(
            matchedRuleId = matched,
            currentAction = curAction,
            candidateAction = candAction,
            changed = curAction != candAction,
        )
    }

    fun parseHistory(data: JSONObject?): List<TrafficPolicyRevisionView> {
        val entries = data?.optJSONArray("entries") ?: data?.optJSONArray("revisions") ?: return emptyList()
        return (0 until entries.length()).mapNotNull { idx ->
            val obj = entries.optJSONObject(idx) ?: return@mapNotNull null
            val rev = obj.longOrNull("revision") ?: return@mapNotNull null
            TrafficPolicyRevisionView(
                revision = rev,
                action = obj.stringOrNull("action") ?: "apply",
                timestampUnixMs = obj.longOrNull("recorded_at_unix_ms") ?: obj.longOrNull("timestamp_unix_ms"),
            )
        }
    }

    private fun encodeAction(kind: String, target: String?): JSONObject {
        val obj = JSONObject().put("kind", kind)
        if ((kind == "exit" || kind == "network") && !target.isNullOrBlank()) {
            obj.put("value", target.trim())
        }
        return obj
    }

    private fun looksLikeIp(value: String): Boolean {
        if (value.contains(':')) return true
        val parts = value.split('.')
        return parts.size == 4 && parts.all { p -> p.toIntOrNull()?.let { it in 0..255 } == true }
    }
}
