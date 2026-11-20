package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.FileCapabilityView
import cc.xlei.fuckipv6.android.bridge.NetworkLabel
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeFileCapabilitySnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkMembershipSnapshot

/**
 * Read-only projection of the authoritative File capability for the Android
 * surface.
 *
 * The projection intentionally mirrors what GPUI and WebUI already show: the
 * node reports one capability state per owner-local share, plus the network it
 * is bound to, the access epoch, the share revision and the remaining disk
 * space. It never carries a share root, an absolute path or a file name, so the
 * Android surface cannot leak more than the desktop surfaces already expose.
 *
 * Transfers themselves stay in the desktop surfaces: the Android app has no
 * node bridge yet, so this page must not offer browse/upload/download actions
 * it cannot honour. [transferSurfaceKey] points at the existing note that says
 * exactly that.
 */
data class FilePageData(
    val stateKey: String,
    val stateTone: String,
    val networkLabel: String?,
    val activeTransfers: Int,
    val availableBytes: Long?,
    val formattedAvailable: String?,
    val accessEpoch: Long?,
    val shareRevision: Long?,
    val rootErrorCode: String?,
    val transferSurfaceKey: String,
) {
    val showsTransferActions: Boolean
        get() = false

    companion object {
        const val TRANSFER_SURFACE_KEY = "file.page.local_state.note"

        private val SUPPORTED_STATES = setOf(
            "disabled",
            "starting",
            "enabled",
            "degraded",
            "unavailable",
            "unsupported",
            "unknown",
        )

        fun from(
            capability: NodeFileCapabilitySnapshot?,
            memberships: List<NodeNetworkMembershipSnapshot> = emptyList(),
        ): FilePageData {
            val rawState = capability?.state?.trim()?.lowercase()
            val state = when {
                rawState == null || rawState.isEmpty() -> "disabled"
                rawState in SUPPORTED_STATES -> rawState
                // An unrecognised wire value must never be rendered as usable.
                else -> "unknown"
            }
            val networkId = capability?.network_id?.trim()
            val networkLabel = networkId
                ?.takeIf { it.isNotEmpty() }
                ?.let { id ->
                    memberships.firstOrNull { it.network_id == id }?.network_name
                        ?: memberships.firstOrNull { it.network_name == id }?.network_name
                }
            val available = capability?.available_bytes?.takeIf { it >= 0 }
            return FilePageData(
                stateKey = "status.capability.$state",
                stateTone = when (state) {
                    "enabled" -> "good"
                    "degraded", "starting", "unavailable" -> "warn"
                    else -> "neutral"
                },
                networkLabel = networkLabel,
                activeTransfers = capability?.active_transfers?.toInt() ?: 0,
                availableBytes = available,
                formattedAvailable = available?.let(::formatBytes),
                accessEpoch = capability?.access_epoch?.takeIf { it >= 0 },
                shareRevision = capability?.share_revision?.takeIf { it >= 0 },
                rootErrorCode = capability?.root_error
                    ?.trim()
                    ?.takeIf { it.isNotEmpty() }
                    ?.let(::stableErrorCode),
                transferSurfaceKey = TRANSFER_SURFACE_KEY,
            )
        }

        fun from(
            capability: FileCapabilityView?,
            networks: List<NetworkLabel>,
        ): FilePageData = from(
            capability = capability?.let {
                NodeFileCapabilitySnapshot(
                    it.accessEpoch,
                    it.activeTransfers,
                    it.availableBytes,
                    it.networkId,
                    it.rootError,
                    it.shareRevision,
                    it.state,
                )
            },
            memberships = networks.map { label ->
                NodeNetworkMembershipSnapshot(
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    label.networkId,
                    label.networkName,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                    null,
                )
            },
        )

        /**
         * Keep only a stable machine-readable code. Owner-local errors can carry
         * host paths in their prose; the Android surface must never render
         * those, so anything that is not a dotted code collapses to
         * `file.error.unspecified`.
         */
        fun stableErrorCode(message: String): String {
            val trimmed = message.trim()
            val looksLikeCode = trimmed.isNotEmpty() &&
                trimmed.length <= 96 &&
                trimmed.all { it.isLetterOrDigit() || it in "._-" } &&
                trimmed.contains('.') &&
                !trimmed.startsWith(".") &&
                !trimmed.endsWith(".")
            return if (looksLikeCode) trimmed else "file.error.unspecified"
        }

        /** Binary units with one decimal, deterministic for tests. */
        fun formatBytes(bytes: Long): String {
            if (bytes < 0) return "--"
            if (bytes < 1024) return "$bytes B"
            val units = listOf("KiB", "MiB", "GiB", "TiB", "PiB")
            var value = bytes.toDouble() / 1024.0
            var unit = 0
            while (value >= 1024.0 && unit < units.lastIndex) {
                value /= 1024.0
                unit += 1
            }
            val rounded = kotlin.math.round(value * 10.0) / 10.0
            val text = if (rounded >= 100.0) {
                rounded.toLong().toString()
            } else {
                String.format(java.util.Locale.US, "%.1f", rounded)
            }
            return "$text ${units[unit]}"
        }
    }
}
