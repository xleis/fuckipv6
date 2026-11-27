package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.RoutingSummaryView
import cc.xlei.fuckipv6.android.bridge.TrafficPolicyDocument
import cc.xlei.fuckipv6.android.bridge.TrafficPolicyPreviewView
import cc.xlei.fuckipv6.android.bridge.TrafficPolicyRevisionView
import cc.xlei.fuckipv6.android.bridge.TrafficPolicySurface
import cc.xlei.fuckipv6.android.bridge.TrafficPolicyTestView
import cc.xlei.fuckipv6.android.bridge.TrafficRuleItem
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Applied routing summary + structured Traffic Policy editor/tester/rollback
 * surface (W11-08 #7 / W16-05 #5).
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun RoutingContent(
    contracts: SharedContracts,
    locale: Locale,
    summaries: List<RoutingSummaryView>?,
    networkName: String? = null,
    onUpdateRouting: ((mode: String, exitNode: String?, isExitNode: Boolean) -> Unit)? = null,
    trafficPolicy: TrafficPolicyDocument? = null,
    trafficPolicyPreview: TrafficPolicyPreviewView? = null,
    trafficPolicyTest: TrafficPolicyTestView? = null,
    trafficPolicyHistory: List<TrafficPolicyRevisionView> = emptyList(),
    onLoadTrafficPolicy: (() -> Unit)? = null,
    onPreviewTrafficPolicy: ((defaultKind: String, defaultTarget: String?, rules: List<TrafficRuleItem>) -> Unit)? = null,
    onApplyTrafficPolicy: ((defaultKind: String, defaultTarget: String?, rules: List<TrafficRuleItem>, confirmRisk: Boolean) -> Unit)? = null,
    onTestTrafficPolicy: ((destination: String, defaultKind: String, defaultTarget: String?, rules: List<TrafficRuleItem>) -> Unit)? = null,
    onRollbackTrafficPolicy: ((targetRevision: Long) -> Unit)? = null,
) {
    var selectedMode by remember(networkName) { mutableStateOf("mesh_only") }
    var exitNode by remember(networkName) { mutableStateOf("") }
    var isExitNode by remember(networkName) { mutableStateOf(false) }
    var confirm by remember { mutableStateOf(false) }

    // Structured TrafficPolicy editing state synchronized from the authoritative policy document.
    var draftDefaultKind by remember(trafficPolicy?.revision) {
        mutableStateOf(trafficPolicy?.defaultActionKind ?: "direct")
    }
    var draftDefaultTarget by remember(trafficPolicy?.revision) {
        mutableStateOf(trafficPolicy?.defaultActionTarget.orEmpty())
    }
    var draftRules by remember(trafficPolicy?.revision) {
        mutableStateOf(trafficPolicy?.rules.orEmpty())
    }
    var newRuleDomain by remember { mutableStateOf("") }
    var newRuleAction by remember { mutableStateOf("exit") }
    var newRuleTarget by remember { mutableStateOf("") }
    var newRuleFixedExit by remember { mutableStateOf(false) }
    var testDestination by remember { mutableStateOf("www.google.com") }
    var confirmApplyPolicy by remember { mutableStateOf(false) }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Card(
            modifier = Modifier
                .fillMaxWidth()
                .testTag("routing-summaries"),
        ) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "nav.routing"),
                    style = MaterialTheme.typography.titleMedium,
                )
                when {
                    summaries == null -> Text(
                        modifier = Modifier.testTag("routing-unobserved"),
                        text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    summaries.isEmpty() -> Text(
                        modifier = Modifier.testTag("routing-empty"),
                        text = contracts.i18n.translate(locale, "common.no_data"),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    else -> summaries.forEach { summary ->
                        Column(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("routing-${summary.networkName}"),
                            verticalArrangement = Arrangement.spacedBy(2.dp),
                        ) {
                            Text(summary.networkName, style = MaterialTheme.typography.bodyLarge)
                            Text(
                                text = buildString {
                                    append(summary.defaultActionKind ?: "unknown")
                                    summary.defaultActionValue?.let { append(" → $it") }
                                    append(" · rules ")
                                    append(summary.enabledRuleCount)
                                    append(" · ")
                                    append(summary.status ?: "unknown")
                                    summary.lastAppliedRevision?.let { append(" · rev $it") }
                                    summary.problemCode?.let { append(" · $it") }
                                },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
                if (onUpdateRouting != null && !networkName.isNullOrBlank()) {
                    Text(
                        text = contracts.i18n.translate(locale, "action.update_routing"),
                        style = MaterialTheme.typography.labelLarge,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        for (mode in cc.xlei.fuckipv6.android.bridge.RoutingSurface.MODES) {
                            FilterChip(
                                modifier = Modifier
                                    .heightIn(min = 48.dp)
                                    .testTag("routing-mode-$mode"),
                                selected = mode == selectedMode,
                                onClick = { selectedMode = mode },
                                label = {
                                    Text(contracts.i18n.translate(locale, "choice.routing.$mode"))
                                },
                            )
                        }
                    }
                    OutlinedTextField(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("routing-exit-input"),
                        value = exitNode,
                        onValueChange = { exitNode = it },
                        label = { Text(contracts.i18n.translate(locale, "nav.exit_node")) },
                        singleLine = true,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(
                            text = contracts.i18n.translate(locale, "action.set_local_exit_capability"),
                            style = MaterialTheme.typography.bodySmall,
                        )
                        Switch(
                            modifier = Modifier.testTag("routing-is-exit"),
                            checked = isExitNode,
                            onCheckedChange = { isExitNode = it },
                        )
                    }
                    TextButton(
                        modifier = Modifier
                            .heightIn(min = 48.dp)
                            .testTag("routing-submit"),
                        onClick = { confirm = true },
                    ) {
                        Text(contracts.i18n.translate(locale, "action.update_routing"))
                    }
                }
            }
        }

        // Structured TrafficPolicy editor / tester / preview / apply / rollback card (W11-08 #7)
        if (onLoadTrafficPolicy != null || trafficPolicy != null) {
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("traffic-policy-card"),
            ) {
                Column(
                    modifier = Modifier.padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        modifier = Modifier.semantics { heading() },
                        text = contracts.i18n.translate(locale, "android.traffic_policy.title"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        modifier = Modifier.testTag("traffic-policy-revision"),
                        text = trafficPolicy?.let {
                            "rev ${it.revision} · default=${it.defaultActionKind} · rules=${draftRules.size}"
                        } ?: contracts.i18n.translate(locale, "peers.unobserved.detail"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    if (onLoadTrafficPolicy != null) {
                        OutlinedButton(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("traffic-policy-load"),
                            onClick = onLoadTrafficPolicy,
                        ) {
                            Text(contracts.i18n.translate(locale, "android.traffic_policy.load"))
                        }
                    }

                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        for (kind in TrafficPolicySurface.ACTION_KINDS) {
                            FilterChip(
                                modifier = Modifier
                                    .heightIn(min = 48.dp)
                                    .testTag("traffic-policy-default-$kind"),
                                selected = draftDefaultKind == kind,
                                onClick = { draftDefaultKind = kind },
                                label = { Text(kind) },
                            )
                        }
                    }
                    if (draftDefaultKind == "exit" || draftDefaultKind == "network") {
                        OutlinedTextField(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("traffic-policy-default-target"),
                            value = draftDefaultTarget,
                            onValueChange = { draftDefaultTarget = it },
                            label = { Text(contracts.i18n.translate(locale, "nav.exit_node")) },
                            singleLine = true,
                        )
                    }

                    draftRules.forEachIndexed { index, rule ->
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("traffic-policy-rule-${rule.id}"),
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            Column(modifier = Modifier.weight(1f)) {
                                Text(
                                    text = "#${index + 1} ${rule.name} (${rule.matcherKind}=${rule.matcherValue})",
                                    style = MaterialTheme.typography.bodySmall,
                                )
                                Text(
                                    text = "${rule.actionKind}${rule.actionTarget?.let { " → $it" }.orEmpty()}${if (rule.fixedExit) " · fixed-exit" else ""}",
                                    style = MaterialTheme.typography.labelSmall,
                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                )
                            }
                            Row {
                                if (index > 0) {
                                    TextButton(
                                        modifier = Modifier.testTag("rule-up-${rule.id}"),
                                        onClick = {
                                            val copy = draftRules.toMutableList()
                                            val item = copy.removeAt(index)
                                            copy.add(index - 1, item)
                                            draftRules = copy
                                        },
                                    ) { Text("↑") }
                                }
                                TextButton(
                                    modifier = Modifier.testTag("rule-remove-${rule.id}"),
                                    onClick = { draftRules = draftRules.filterNot { it.id == rule.id } },
                                ) { Text("✕") }
                            }
                        }
                    }

                    HorizontalDivider()
                    OutlinedTextField(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("traffic-policy-new-domain"),
                        value = newRuleDomain,
                        onValueChange = { newRuleDomain = it },
                        label = { Text("domain / CIDR") },
                        singleLine = true,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        for (kind in listOf("exit", "direct", "reject")) {
                            FilterChip(
                                modifier = Modifier.heightIn(min = 48.dp),
                                selected = newRuleAction == kind,
                                onClick = { newRuleAction = kind },
                                label = { Text(kind) },
                            )
                        }
                    }
                    if (newRuleAction == "exit") {
                        OutlinedTextField(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("traffic-policy-new-target"),
                            value = newRuleTarget,
                            onValueChange = { newRuleTarget = it },
                            label = { Text(contracts.i18n.translate(locale, "nav.exit_node")) },
                            singleLine = true,
                        )
                    }
                    FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        OutlinedButton(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("traffic-policy-add-rule"),
                            onClick = {
                                val matcher = newRuleDomain.trim()
                                if (matcher.isNotEmpty()) {
                                    val kind = if (matcher.contains('/')) "ip_cidr" else "domain_suffix"
                                    val rule = TrafficRuleItem(
                                        id = "rule-${draftRules.size + 1}",
                                        name = matcher,
                                        enabled = true,
                                        matcherKind = kind,
                                        matcherValue = matcher,
                                        actionKind = newRuleAction,
                                        actionTarget = newRuleTarget.trim().ifEmpty { null },
                                        fixedExit = newRuleFixedExit,
                                    )
                                    draftRules = draftRules + rule
                                    newRuleDomain = ""
                                }
                            },
                        ) {
                            Text(contracts.i18n.translate(locale, "android.traffic_policy.add_rule"))
                        }
                        if (onPreviewTrafficPolicy != null) {
                            OutlinedButton(
                                modifier = Modifier
                                    .heightIn(min = 48.dp)
                                    .testTag("traffic-policy-preview"),
                                onClick = {
                                    onPreviewTrafficPolicy(
                                        draftDefaultKind,
                                        draftDefaultTarget.trim().ifEmpty { null },
                                        draftRules,
                                    )
                                },
                            ) {
                                Text(contracts.i18n.translate(locale, "android.traffic_policy.preview"))
                            }
                        }
                        if (onApplyTrafficPolicy != null) {
                            Button(
                                modifier = Modifier
                                    .heightIn(min = 48.dp)
                                    .testTag("traffic-policy-apply"),
                                onClick = { confirmApplyPolicy = true },
                            ) {
                                Text(contracts.i18n.translate(locale, "android.traffic_policy.apply"))
                            }
                        }
                    }

                    trafficPolicyPreview?.let { preview ->
                        Text(
                            modifier = Modifier.testTag("traffic-policy-preview-result"),
                            text = "rev ${preview.currentRevision} → ${preview.candidateRevision} · changes=${preview.changeCount}${preview.riskSummary?.let { " · risk: $it" }.orEmpty()}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }

                    HorizontalDivider()
                    OutlinedTextField(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("traffic-policy-test-input"),
                        value = testDestination,
                        onValueChange = { testDestination = it },
                        label = { Text(contracts.i18n.translate(locale, "android.traffic_policy.test")) },
                        singleLine = true,
                    )
                    if (onTestTrafficPolicy != null) {
                        OutlinedButton(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("traffic-policy-test-submit"),
                            onClick = {
                                onTestTrafficPolicy(
                                    testDestination,
                                    draftDefaultKind,
                                    draftDefaultTarget.trim().ifEmpty { null },
                                    draftRules,
                                )
                            },
                        ) {
                            Text(contracts.i18n.translate(locale, "android.traffic_policy.test"))
                        }
                    }
                    trafficPolicyTest?.let { result ->
                        Text(
                            modifier = Modifier.testTag("traffic-policy-test-result"),
                            text = "current=${result.currentAction} → candidate=${result.candidateAction}${result.matchedRuleId?.let { " · rule=$it" }.orEmpty()}",
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }

                    if (trafficPolicyHistory.isNotEmpty() && onRollbackTrafficPolicy != null) {
                        HorizontalDivider()
                        Text(
                            text = contracts.i18n.translate(locale, "android.traffic_policy.history"),
                            style = MaterialTheme.typography.labelLarge,
                        )
                        trafficPolicyHistory.take(5).forEach { rev ->
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.SpaceBetween,
                            ) {
                                Text("rev ${rev.revision} (${rev.action})", style = MaterialTheme.typography.bodySmall)
                                TextButton(
                                    modifier = Modifier.testTag("traffic-policy-rollback-${rev.revision}"),
                                    onClick = { onRollbackTrafficPolicy(rev.revision) },
                                ) {
                                    Text(contracts.i18n.translate(locale, "android.traffic_policy.rollback"))
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.update_routing.title"))
            },
            text = {
                Text(
                    contracts.i18n.translate(locale, "confirm.update_routing.consequence"),
                    style = MaterialTheme.typography.bodySmall,
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("routing-confirm"),
                    onClick = {
                        confirm = false
                        onUpdateRouting?.invoke(
                            selectedMode,
                            exitNode.trim().ifEmpty { null },
                            isExitNode,
                        )
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "action.update_routing"))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirm = false }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }

    if (confirmApplyPolicy) {
        AlertDialog(
            onDismissRequest = { confirmApplyPolicy = false },
            title = {
                Text(contracts.i18n.translate(locale, "android.traffic_policy.apply"))
            },
            text = {
                Text(
                    contracts.i18n.translate(locale, "confirm.update_routing.consequence"),
                    style = MaterialTheme.typography.bodySmall,
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("traffic-policy-apply-confirm"),
                    onClick = {
                        confirmApplyPolicy = false
                        onApplyTrafficPolicy?.invoke(
                            draftDefaultKind,
                            draftDefaultTarget.trim().ifEmpty { null },
                            draftRules,
                            true,
                        )
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "android.traffic_policy.apply"))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmApplyPolicy = false }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}
