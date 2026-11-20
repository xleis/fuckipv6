package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceAuditSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthSnapshot

/**
 * Relay-relevant slice of the authoritative snapshot.
 *
 * The DTOs are the generated contract types so the existing Relay page and its
 * projection keep working unchanged; the bridge only decides which parts of the
 * JSON are trustworthy enough to hand over. A health record without a status or
 * lifecycle is dropped entirely rather than rendered as "healthy".
 */
data class RelaySurface(
    val networkName: String,
    val relayEnabled: Boolean,
    val health: RelayHealthSnapshot?,
    val governance: RelayGovernanceAuditSnapshot?,
    val permissions: Set<NodePermission>,
)

/** Everything the Android surfaces read from one poll. */
data class ParsedSnapshot(
    val file: SnapshotFileView,
    val relay: RelaySurface?,
    val coordinator: CoordinatorSurface? = null,
)