package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

@Composable
fun CoordinatorPageContent(
    contracts: SharedContracts,
    locale: Locale,
    data: CoordinatorPageData,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "runtime.section.control_plane"),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = data.networkName,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                text = listOf(
                    contracts.i18n.translate(locale, data.stateKey),
                    contracts.i18n.translate(
                        locale,
                        if (data.governanceWritable) {
                            "runtime.control_plane.governance_writable"
                        } else {
                            "runtime.control_plane.governance_read_only"
                        },
                    ),
                ).joinToString(" · "),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                text = listOf(
                    contracts.i18n.translate(locale, "runtime.control_plane.votes"),
                    "${data.receivedVotes}/${data.requiredVotes}",
                    contracts.i18n.translate(locale, "runtime.control_plane.participants"),
                    data.participantCount.toString(),
                    contracts.i18n.translate(locale, "runtime.coordinator.reason"),
                    data.reasonCode,
                ).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("coordinator-current-leader"),
                text = "${contracts.i18n.translate(locale, "runtime.coordinator.leader")}: " +
                    (data.currentLeader
                        ?: contracts.i18n.translate(locale, "runtime.coordinator.leader.none")),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                modifier = Modifier.testTag("coordinator-latest-term"),
                text = data.latestTerm?.let { term ->
                    listOf(
                        contracts.i18n.translate(locale, "runtime.coordinator.latest_term"),
                        "${contracts.i18n.translate(locale, "runtime.coordinator.term")} ${term.term}",
                        "${contracts.i18n.translate(locale, "runtime.coordinator.fencing_token")} ${term.fencingToken}",
                        "${contracts.i18n.translate(locale, "runtime.coordinator.leader")} ${term.leader}",
                        contracts.i18n.translate(locale, term.phaseKey),
                    ).joinToString(" · ")
                } ?: contracts.i18n.translate(locale, "runtime.coordinator.latest_term.none"),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "runtime.coordinator.candidates"),
                style = MaterialTheme.typography.titleSmall,
            )
            if (data.candidates.isEmpty()) {
                Text(
                    modifier = Modifier.testTag("coordinator-candidates-empty"),
                    text = contracts.i18n.translate(locale, "runtime.coordinator.candidates.none"),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                data.candidates.forEach { candidate ->
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("coordinator-candidate-${candidate.nodeId}"),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(
                            text = candidate.nodeId,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Text(
                            text = listOf(
                                contracts.i18n.translate(locale, candidate.healthKey),
                                contracts.i18n.translate(locale, "runtime.coordinator.eligible"),
                                contracts.i18n.translate(
                                    locale,
                                    if (candidate.eligibleForElection) {
                                        "runtime.coordinator.eligible.yes"
                                    } else {
                                        "runtime.coordinator.eligible.no"
                                    },
                                ),
                                contracts.i18n.translate(locale, "runtime.coordinator.reason"),
                                candidate.reasonCode,
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}
