package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.RelaySurface
import cc.xlei.fuckipv6.android.bridge.RecoveryStatusView
import cc.xlei.fuckipv6.android.bridge.SnapshotFileView
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryListCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryStatusCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeExecuteAdminRecoveryCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeRevokeAdminRecoveryCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSubmitAdminRecoveryCommand

/**
 * Callbacks the shell needs to hand to the surfaces. Keeping them behind one
 * interface means the page data can be assembled in one place, and the shell
 * mount stays a single call.
 */
interface SurfaceHandlers {
    fun setRelayRole(enabled: Boolean)
    fun submitRecovery(command: NodeSubmitAdminRecoveryCommand)
    fun getRecoveryStatus(command: NodeAdminRecoveryStatusCommand)
    fun listRecoveries(command: NodeAdminRecoveryListCommand)
    fun revokeRecovery(command: NodeRevokeAdminRecoveryCommand)
    fun executeRecovery(command: NodeExecuteAdminRecoveryCommand)
    fun saveNodeApi(url: String, token: String): SettingsValidation
    fun clearNodeApi()
}

/**
 * Everything the shell renders, resolved from bridge state and controller
 * state. No field is inferred: when a slice is missing the page shows its own
 * "unobserved"/"unavailable" state instead of a default that looks live.
 */
data class AppSurfaces(
    val files: FilePageData? = null,
    val filesFailureKey: String? = null,
    val relay: RelayPageData? = null,
    val coordinator: CoordinatorPageData? = null,
    val adminRecovery: AdminRecoveryPageData? = null,
    val settings: SettingsPageData? = null,
    val fileBrowse: FileWorkspaceState? = null,
    val transfers: FileTransferState? = null,
    val filesRevision: Long? = null,
) {
    val anySurfaceReady: Boolean
        get() = files != null || relay != null || coordinator != null || adminRecovery != null
}

object SurfaceResolver {
    fun resolve(
        bridgeState: BridgeState,
        recoveries: List<RecoveryStatusView>,
        browse: FileWorkspaceState,
        transfers: FileTransferState,
        settingsUrl: String?,
        settingsHasToken: Boolean,
        configured: Boolean,
        handlers: SurfaceHandlers,
    ): AppSurfaces {
        val base = AppSurfaces(
            settings = nodeApiSettingsData(
                url = settingsUrl,
                hasToken = settingsHasToken,
                connected = configured,
                onSave = handlers::saveNodeApi,
                onClear = handlers::clearNodeApi,
            ),
            fileBrowse = browse,
            transfers = transfers,
        )
        return when (bridgeState) {
            BridgeState.NotConfigured -> base
            is BridgeState.Failed -> base.copy(filesFailureKey = bridgeState.failure.uiStateKey)
            is BridgeState.Ready -> base.copy(
                files = bridgeState.data,
                filesRevision = bridgeState.snapshotRevision,
                relay = bridgeState.relay?.let { relayData(it, handlers) },
                coordinator = bridgeState.coordinator?.let(CoordinatorPageData::fromBridge),
                adminRecovery = adminData(bridgeState, recoveries, handlers),
            )
        }
    }

    private fun relayData(surface: RelaySurface, handlers: SurfaceHandlers): RelayPageData =
        RelayPageData(
            networkName = surface.networkName,
            relayEnabled = surface.relayEnabled,
            health = surface.health,
            governance = surface.governance,
            grantedPermissions = surface.permissions,
            backendAvailable = true,
            onSetRelayRole = handlers::setRelayRole,
        )

    private fun adminData(
        state: BridgeState.Ready,
        recoveries: List<RecoveryStatusView>,
        handlers: SurfaceHandlers,
    ): AdminRecoveryPageData? {
        val networkName = state.relay?.networkName
            ?: state.networks.firstOrNull()?.networkName
            ?: return null
        val networkId = state.networks.firstOrNull { it.networkName == networkName }?.networkId
        return AdminRecoveryPageData.fromBridge(
            networkName = networkName,
            networkId = networkId,
            recoveries = recoveries,
            onSubmit = handlers::submitRecovery,
            onGetStatus = handlers::getRecoveryStatus,
            onList = handlers::listRecoveries,
            onRevoke = handlers::revokeRecovery,
            onExecute = handlers::executeRecovery,
        )
    }

    /** Convenience for callers that only have a File slice. */
    fun fileDataOnly(view: SnapshotFileView): FilePageData = view.toPageData()
}