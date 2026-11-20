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

/**
 * Coordinator control-plane card.
 *
 * The headline answers three questions a reader actually has: is governance
 * usable, who is leading, and is a term in flight. Candidate ids, fencing
 * tokens, quorum counts and wire reason codes are protocol material, so they
 * live in the collapsed technical block instead of the headline.
 */
@Composable
fun CoordinatorPageContent(
    contracts: SharedContracts,
    locale: Locale,
    data: CoordinatorPageData,
) {
    val notAvailable = contracts.i18n.translate(locale, "common.not_available")
    val currentLeader = abbreviateId(data.currentLeader)

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
                text = contracts.i18n.translate(locale, data.stateKey),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(
                text = listOf(
                    contracts.i18n.translate(
                        locale,
                        if (data.governanceWritable) {
                            "runtime.control_plane.governance_writable"
                        } else {
                            "runtime.control_plane.governance_read_only"
                        },
                    ),
                    contracts.i18n.translate(
                        locale,
                        "runtime.control_plane.votes",
                    ) + " ${data.receivedVotes}/${data.requiredVotes}",
                ).joinToString(" · "),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("coordinator-current-leader"),
                text = "${contracts.i18n.translate(locale, "runtime.coordinator.leader")}: " +
                    (currentLeader ?: contracts.i18n.translate(
                        locale,
                        "runtime.coordinator.leader.none",
                    )),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                modifier = Modifier.testTag("coordinator-latest-term"),
                text = data.latestTerm?.let { term ->
                    listOf(
                        contracts.i18n.translate(locale, "runtime.coordinator.latest_term"),
                        "${contracts.i18n.translate(locale, "runtime.coordinator.term")} ${term.term}",
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
                            text = abbreviateId(candidate.nodeId) ?: candidate.nodeId,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Text(
                            text = listOf(
                                contracts.i18n.translate(locale, candidate.healthKey),
                                contracts.i18n.translate(
                                    locale,
                                    "runtime.coordinator.eligible",
                                ) + " " + contracts.i18n.translate(
                                    locale,
                                    if (candidate.eligibleForElection) {
                                        "runtime.coordinator.eligible.yes"
                                    } else {
                                        "runtime.coordinator.eligible.no"
                                    },
                                ),
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            TechnicalDetails(
                contracts = contracts,
                locale = locale,
                testTagPrefix = "coordinator-tech-details",
            ) {
                TechValue(
                    label = contracts.i18n.translate(locale, "runtime.coordinator.reason"),
                    value = data.reasonCode,
                )
                TechValue(
                    label = contracts.i18n.translate(
                        locale,
                        "runtime.control_plane.participants",
                    ),
                    value = data.participantCount.toString(),
                )
                TechValue(
                    label = contracts.i18n.translate(locale, "runtime.coordinator.leader"),
                    value = data.currentLeader ?: notAvailable,
                )
                data.latestTerm?.let { term ->
                    TechValue(
                        label = contracts.i18n.translate(
                            locale,
                            "runtime.coordinator.fencing_token",
                        ),
                        value = term.fencingToken.toString(),
                    )
                    TechValue(
                        label = contracts.i18n.translate(locale, "runtime.coordinator.latest_term"),
                        value = listOf(
                            "${contracts.i18n.translate(locale, "runtime.coordinator.term")} ${term.term}",
                            contracts.i18n.translate(locale, term.phaseKey),
                            term.leader,
                        ).joinToString(" · "),
                    )
                }
                data.candidates.forEach { candidate ->
                    TechValue(
                        label = listOfNotNull(
                            abbreviateId(candidate.nodeId),
                            contracts.i18n.translate(locale, candidate.healthKey),
                        ).joinToString(" · "),
                        value = candidate.reasonCode,
                    )
                }
            }
        }
    }
}