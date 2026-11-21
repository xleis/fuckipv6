package cc.xlei.fuckipv6.android

import android.content.Intent
import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import java.util.UUID
import java.util.concurrent.atomic.AtomicLong
import cc.xlei.fuckipv6.android.bridge.AdminRecoverySurface
import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.CommandJson
import cc.xlei.fuckipv6.android.bridge.CommandOutcome
import cc.xlei.fuckipv6.android.bridge.RecoveryStatusView
import cc.xlei.fuckipv6.android.bridge.HttpSnapshotSource
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig
import cc.xlei.fuckipv6.android.bridge.NodeBridge
import cc.xlei.fuckipv6.android.bridge.LiveAction
import cc.xlei.fuckipv6.android.bridge.NodeCommandBridge
import cc.xlei.fuckipv6.android.bridge.NodeCommandClient
import cc.xlei.fuckipv6.android.bridge.FileBrowseClient
import cc.xlei.fuckipv6.android.bridge.MemberDirectoryView
import cc.xlei.fuckipv6.android.bridge.CreateInvitationRequest
import cc.xlei.fuckipv6.android.bridge.InvitationSurface
import cc.xlei.fuckipv6.android.bridge.InvitationView
import cc.xlei.fuckipv6.android.bridge.JoinRequestSurface
import cc.xlei.fuckipv6.android.bridge.JoinRequestView
import cc.xlei.fuckipv6.android.bridge.JoinReviewRequest
import cc.xlei.fuckipv6.android.bridge.MemberLifecycleRequest
import cc.xlei.fuckipv6.android.bridge.MemberLifecycleSurface
import cc.xlei.fuckipv6.android.bridge.MemberSurface
import cc.xlei.fuckipv6.android.bridge.NetworkLabel
import cc.xlei.fuckipv6.android.bridge.ActivitySurface
import cc.xlei.fuckipv6.android.bridge.IdentitySurface
import cc.xlei.fuckipv6.android.bridge.LogsSurface
import cc.xlei.fuckipv6.android.bridge.NetworksSurface
import cc.xlei.fuckipv6.android.bridge.ExitSurface
import cc.xlei.fuckipv6.android.bridge.RoutingSurface
import cc.xlei.fuckipv6.android.bridge.BlacklistSurface
import cc.xlei.fuckipv6.android.bridge.BlacklistView
import cc.xlei.fuckipv6.android.bridge.TopologySurface
import cc.xlei.fuckipv6.android.bridge.PeersSurface
import cc.xlei.fuckipv6.android.bridge.TrafficSurface
import cc.xlei.fuckipv6.android.bridge.MemberView
import cc.xlei.fuckipv6.android.bridge.DirectoryEntry
import cc.xlei.fuckipv6.android.bridge.FileTransferClient
import cc.xlei.fuckipv6.android.bridge.HttpDownloadFetcher
import cc.xlei.fuckipv6.android.bridge.PendingDownload
import cc.xlei.fuckipv6.android.bridge.TransferRequest
import cc.xlei.fuckipv6.android.bridge.NodeEventStream
import cc.xlei.fuckipv6.android.bridge.SnapshotFrameCoalescer
import cc.xlei.fuckipv6.android.bridge.RelaySurface
import cc.xlei.fuckipv6.android.ui.AdminRecoveryPageData
import cc.xlei.fuckipv6.android.ui.CoordinatorPageData
import cc.xlei.fuckipv6.android.ui.DownloadSaver
import cc.xlei.fuckipv6.android.ui.FileBrowseActions
import cc.xlei.fuckipv6.android.ui.FileBrowseController
import cc.xlei.fuckipv6.android.ui.InvitationListContent
import cc.xlei.fuckipv6.android.ui.JoinRequestListContent
import cc.xlei.fuckipv6.android.ui.MemberListContent
import cc.xlei.fuckipv6.android.ui.NetworkSelection
import cc.xlei.fuckipv6.android.ui.FileTransferController
import cc.xlei.fuckipv6.android.ui.FileTransferNaming
import cc.xlei.fuckipv6.android.ui.RelayPageData
import cc.xlei.fuckipv6.android.ui.SettingsPageData
import cc.xlei.fuckipv6.android.ui.SettingsValidation
import cc.xlei.fuckipv6.android.ui.nodeApiSettingsData
import cc.xlei.fuckipv6.android.platform.SafDownloadSink
import cc.xlei.fuckipv6.android.platform.displayName
import cc.xlei.fuckipv6.android.platform.openUploadSource
import cc.xlei.fuckipv6.android.platform.uploadLength
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import cc.xlei.fuckipv6.android.contract.SharedContractLoader
import cc.xlei.fuckipv6.android.ui.ContractFailureScreen
import cc.xlei.fuckipv6.android.ui.FuckIpv6App
import cc.xlei.fuckipv6.android.ui.OnboardingPage
import cc.xlei.fuckipv6.android.ui.OnboardingProjection
import cc.xlei.fuckipv6.android.ui.OnboardingStep
import cc.xlei.fuckipv6.android.ui.FuckIpv6Theme

class MainActivity : ComponentActivity() {
    private val bridgeScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var pollJob: Job? = null
    private var bridge: NodeBridge? = null
    private var commandBridge: NodeCommandBridge? = null
    private var recoveryBridge: NodeCommandBridge? = null
    private val requestSequence = AtomicLong(System.currentTimeMillis())
    private val recoveries = MutableStateFlow<List<RecoveryStatusView>>(emptyList())
    private val adminPage = MutableStateFlow<AdminRecoveryPageData?>(null)
    private var eventJob: Job? = null
    private var refreshNetwork: String? = null
    private var browseController: FileBrowseController? = null
    private var transferController: FileTransferController? = null
    private var downloadSaver: DownloadSaver? = null
    private var apiConfig: NodeApiConfig? = null
    private var pendingDownloadName: String? = null
    private var memberBridge: NodeCommandBridge? = null
    private val members = MutableStateFlow<MemberDirectoryView?>(null)
    private val joinRequests = MutableStateFlow<List<JoinRequestView>?>(null)
    private val invitations = MutableStateFlow<List<InvitationView>?>(null)
    private val issuedInvitationCode = MutableStateFlow<String?>(null)
    private val blacklist = MutableStateFlow<BlacklistView?>(null)
    private val localExit = MutableStateFlow<cc.xlei.fuckipv6.android.bridge.LocalExitView?>(null)
    private var blacklistBridge: NodeCommandBridge? = null
    private var routingBridge: NodeCommandBridge? = null
    private var exitBridge: NodeCommandBridge? = null
    private var memberNetworkId: String? = null
    private val selectedNetwork = MutableStateFlow<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The shipped palette is cream paper in both system modes, so the
        // system bars always need dark glyphs instead of following the OS theme.
        enableEdgeToEdge(
            statusBarStyle = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT),
            navigationBarStyle = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT),
        )
        val contracts = runCatching { SharedContractLoader.load(assets) }

        val apiConfig = loadApiConfig(intent)
        val node = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeBridge(HttpSnapshotSource(it)) }
            ?.also { candidate ->
                bridge = candidate
                pollJob = candidate.start(bridgeScope)
            }
        commandBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
        this.apiConfig = apiConfig
        if (apiConfig != null && apiConfig.isValid) {
            downloadSaver = DownloadSaver(HttpDownloadFetcher(apiConfig), bridgeScope)
            browseController = FileBrowseController(
                FileBrowseClient(apiConfig),
                bridgeScope,
            ).also { controller -> controller.loadTargets() }
            transferController = FileTransferController(
                FileTransferClient(apiConfig),
                bridgeScope,
            ).also { controller -> controller.refresh() }
        }
        memberBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
            ?.also { bridge ->
                bridgeScope.launch {
                    bridge.lastOutcome().collect { outcome ->
                        if (outcome !is CommandOutcome.Succeeded) return@collect
                        when (outcome.outcomeStatus) {
                            "network_members" ->
                                members.value = MemberSurface.parseDirectory(outcome.data)
                            "member_lifecycle_changed" -> {
                                // The change is authoritative; re-read the roster
                                // instead of patching it locally.
                                memberNetworkId?.let(::refreshMembers)
                            }
                            "join_requests" ->
                                joinRequests.value = JoinRequestSurface.parseList(outcome.data)
                            "join_requests_reviewed" -> {
                                memberNetworkId?.let(::refreshJoinRequests)
                                memberNetworkId?.let(::refreshMembers)
                            }
                            "invitations" ->
                                invitations.value = InvitationSurface.parseList(outcome.data)
                            "invitation_created" -> {
                                issuedInvitationCode.value =
                                    InvitationSurface.parseIssued(outcome.data)?.code
                                memberNetworkId?.let(::refreshInvitations)
                            }
                            "invitation_revoked" -> memberNetworkId?.let(::refreshInvitations)
                        }
                    }
                }
            }
        blacklistBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
            ?.also { bridge ->
                bridgeScope.launch {
                    bridge.lastOutcome().collect { outcome ->
                        if (outcome is CommandOutcome.Succeeded &&
                            outcome.outcomeStatus == "blacklist"
                        ) {
                            BlacklistSurface.parse(outcome.data)?.let { blacklist.value = it }
                        }
                    }
                }
            }
        exitBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
            ?.also { bridge ->
                bridgeScope.launch {
                    bridge.lastOutcome().collect { outcome ->
                        if (outcome is CommandOutcome.Succeeded &&
                            outcome.outcomeStatus == "local_exit_capability"
                        ) {
                            // The command returns the authoritative projection; apply
                            // it and refresh the snapshot so nothing else drifts.
                            outcome.data?.let { data ->
                                runCatching { ExitSurface.parseLocalExitObject(data) }
                                    .getOrNull()
                                    ?.let { localExit.value = it }
                            }
                            node?.pollOnce()
                        }
                    }
                }
            }
        routingBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
            ?.also { bridge ->
                bridgeScope.launch {
                    bridge.lastOutcome().collect { outcome ->
                        if (outcome is CommandOutcome.Succeeded &&
                            outcome.outcomeStatus == "completed"
                        ) {
                            // Routing answers with a message, not a projection:
                            // re-read the snapshot instead of parsing prose.
                            node?.pollOnce()
                        }
                    }
                }
            }
        recoveryBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
            ?.also { adminBridge ->
                bridgeScope.launch {
                    adminBridge.lastOutcome().collect { outcome ->
                        if (outcome !is CommandOutcome.Succeeded) return@collect
                        when (outcome.outcomeStatus) {
                            "admin_recoveries" -> recoveries.value =
                                AdminRecoverySurface.parseList(outcome.data)
                            "admin_recovery_status" -> {
                                val status = AdminRecoverySurface.parseSingle(outcome.data)
                                if (status != null) {
                                    recoveries.value = listOf(status) + recoveries.value
                                        .filterNot { it.requestId == status.requestId }
                                    refreshNetwork?.let(::refreshRecoveries)
                                }
                            }
                        }
                    }
                }
            }
        // Events only shorten the wait: every frame still triggers one
        // authoritative snapshot pull, and the timed poll stays as fallback.
        if (node != null && apiConfig != null) {
            // Frames collapse into one authoritative pull per burst, and each
            // reconnect resumes from the newest cursor instead of replaying.
            val coalescer = SnapshotFrameCoalescer(
                scope = bridgeScope,
                pull = { bridgeScope.launch { node.pollOnce() } },
            )
            eventJob = bridgeScope.launch {
                while (true) {
                    val stream = NodeEventStream(apiConfig, resumeCursor = coalescer.resumeCursor())
                    val failure = stream.read { frame -> coalescer.onFrame(frame) }
                    if (failure == null) break
                    delay(5_000)
                }
            }
        }
        bridgeScope.launch {
            node?.state()?.collect { state ->
                val ready = state as? BridgeState.Ready ?: return@collect
                // Every surface reads the selected network, not whichever the
                // snapshot happened to list first.
                val chosen = NetworkSelection.resolve(ready.networks, selectedNetwork.value)
                    ?: return@collect
                if (selectedNetwork.value != chosen.networkName) {
                    selectedNetwork.value = chosen.networkName
                }
                val networkName = chosen.networkName
                if (networkName != null) {
                    val networkId = ready.networks
                        .firstOrNull { it.networkName == networkName }
                        ?.networkId
                    adminPage.value = AdminRecoveryPageData.fromBridge(
                        networkName = networkName,
                        networkId = networkId,
                        recoveries = recoveries.value,
                        onSubmit = { command ->
                            submitRecovery(
                                AdminRecoverySurface.submitCommand(
                                    command.network_name,
                                    command.request,
                                ),
                            )
                        },
                        onGetStatus = { command ->
                            submitRecovery(
                                AdminRecoverySurface.statusCommand(
                                    command.network_name,
                                    command.request_id,
                                ),
                            )
                        },
                        onList = { command -> submitRecovery(AdminRecoverySurface.listCommand(command.network_name)) },
                        onRevoke = { command ->
                            submitRecovery(
                                AdminRecoverySurface.revokeCommand(
                                    command.network_name,
                                    command.request_id,
                                    command.reason,
                                ),
                            )
                        },
                        onExecute = { command ->
                            submitRecovery(
                                AdminRecoverySurface.executeCommand(
                                    command.network_name,
                                    command.request_id,
                                ),
                            )
                        },
                    )
                    if (refreshNetwork != networkName) {
                        refreshNetwork = networkName
                        refreshRecoveries(networkName)
                    }
                    if (networkId != null && networkId != memberNetworkId) {
                        memberNetworkId = networkId
                        refreshBlacklist(selectedNetwork.value)
                        refreshMembers(networkId)
                        refreshJoinRequests(networkId)
                        refreshInvitations(networkId)
                    }
                }
            }
        }

        // Hoisted out of the composable: reading the system locale inside
        // composition is not observable and Compose lint rejects it.
        val uiLocale = java.util.Locale.getDefault()
        setContent {
            val bridgeState by remember(node) {
                node?.state() ?: MutableStateFlow<BridgeState>(BridgeState.NotConfigured)
            }.collectAsState()
            val ready = bridgeState as? BridgeState.Ready
            val browse = browseController
            val browseState = browse?.state()?.collectAsState()?.value
            val transferState = transferController?.state()?.collectAsState()?.value
            val uploadPicker = rememberLauncherForActivityResult(
                ActivityResultContracts.OpenDocument(),
            ) { uri -> if (uri != null) uploadPicked(uri) }
            val savePicker = rememberLauncherForActivityResult(
                ActivityResultContracts.CreateDocument("application/octet-stream"),
            ) { uri ->
                val pending = transferController?.state()?.value?.pendingDownload
                if (uri != null && pending != null) {
                    downloadSaver?.save(pending.token, SafDownloadSink(contentResolver, uri))
                }
            }
            val pendingToken = transferState?.pendingDownload?.token
            LaunchedEffect(pendingToken) {
                if (pendingToken != null) {
                    savePicker.launch(pendingDownloadName ?: "download.bin")
                }
            }
            val files = ready?.data
            val filesFailureKey = (bridgeState as? BridgeState.Failed)?.failure?.uiStateKey
            val availableNetworks: List<NetworkLabel> = ready?.networks.orEmpty()
            val selectedNetworkName = selectedNetwork.collectAsState().value
            val relay = ready?.relay
                ?.takeIf { it.networkName == selectedNetworkName }
                ?.let(::relayPageData)
            val coordinator = ready?.coordinator
                ?.takeIf { it.networkName == selectedNetworkName }
                ?.let(CoordinatorPageData::fromBridge)
            val admin = adminPage.collectAsState().value
            val membersState = members.collectAsState().value
            val joinRequestsState = joinRequests.collectAsState().value
            val invitationsState = invitations.collectAsState().value
            val issuedCodeState = issuedInvitationCode.collectAsState().value
            val blacklistState = blacklist.collectAsState().value
            val apiUrl = getSharedPreferences(PREFERENCES, MODE_PRIVATE)
                .getString(KEY_API_URL, null)
            FuckIpv6Theme {
                contracts.fold(
                    onSuccess = {
                        val onboarding = OnboardingProjection.decide(
                            loadApiConfig(intent),
                            bridgeState,
                        )
                        if (onboarding.step != OnboardingStep.READY) {
                            OnboardingPage(
                                contracts = it,
                                locale = uiLocale,
                                state = onboarding,
                                savedUrl = apiUrl,
                                hasSavedToken = hasSavedToken(),
                                onConnect = { url, token -> saveApiConfig(url, token) },
                            )
                            return@fold
                        }
                        FuckIpv6App(
                            contracts = it,
                            relay = relay,
                            coordinator = coordinator,
                            adminRecovery = admin,
                            peers = ready?.peers
                                ?.let { snapshot -> PeersSurface.parse(snapshot, selectedNetworkName.orEmpty()) },
                            activity = ready?.peers?.let { snapshot ->
                                ActivitySurface.parse(snapshot, memberNetworkId)
                            },
                            traffic = ready?.peers?.let { snapshot ->
                                TrafficSurface.parse(snapshot, selectedNetworkName.orEmpty())
                            },
                            logs = ready?.peers?.let(LogsSurface::parse),
                            identity = ready?.peers?.let(IdentitySurface::parse),
                            networks = ready?.peers?.let(NetworksSurface::parse),
                            routing = ready?.peers?.let(RoutingSurface::parse),
                            localExit = localExit.collectAsState().value
                                ?: ready?.peers?.let(ExitSurface::parseLocalExit),
                            exitSelections = ready?.peers?.let(ExitSurface::parseSelections),
                            topology = ready?.peers?.let(TopologySurface::parse),
                            blacklist = blacklistState,
                            onRefreshBlacklist = { refreshBlacklist(selectedNetwork.value) },
                            onUpdateRouting = ::updateRouting,
                            onSetLocalExit = ::setLocalExit,
                            onBanClient = ::banClient,
                            onUnbanClient = ::unbanClient,
                            availableNetworks = availableNetworks,
                            selectedNetworkName = selectedNetworkName,
                            onSelectNetwork = { name -> selectedNetwork.value = name },
                            members = membersState,
                            onRefreshMembers = { memberNetworkId?.let(::refreshMembers) },
                            onMemberLifecycleAction = ::changeMemberLifecycle,
                            joinRequests = joinRequestsState,
                            onJoinReview = ::reviewJoinRequest,
                            onBatchApprove = ::batchApproveJoinRequests,
                            invitations = invitationsState,
                            issuedInvitationCode = issuedCodeState,
                            onCreateInvitation = ::createInvitation,
                            onRevokeInvitation = ::revokeInvitation,
                            files = files,
                            filesFailureKey = filesFailureKey,
                        )
                    },
                    onFailure = { ContractFailureScreen(it.message) },
                )
            }
        }
    }

    /**
     * Builds the Relay page state from the authoritative snapshot and wires the
     * switch to the command channel. The switch does not assume success: the
     * snapshot poll is what flips `relayEnabled` once the operation completes.
     */
    private fun relayPageData(surface: RelaySurface): RelayPageData = RelayPageData(
        networkName = surface.networkName,
        relayEnabled = surface.relayEnabled,
        health = surface.health,
        governance = surface.governance,
        grantedPermissions = surface.permissions,
        // Without a command channel the page must not offer the action at all.
        backendAvailable = commandBridge != null,
        onSetRelayRole = { enabled ->
            val commands = commandBridge
            if (commands == null) {
                Unit
            } else {
                val body = CommandJson.envelope(
                    command = CommandJson.setNetworkRole(
                        networkName = surface.networkName,
                        role = "relay",
                        enabled = enabled,
                    ),
                    requestId = nextRequestId(),
                    idempotencyKey = "relay-${surface.networkName}-$enabled-${UUID.randomUUID()}",
                )
                commands.submit(bridgeScope, body)
                Unit
            }
        },
    )

    /** Refresh the recovery list through the recovery command bridge. */
    private fun refreshRecoveries(networkName: String) {
        submitRecovery(AdminRecoverySurface.listCommand(networkName), networkName)
    }

    /**
     * One command at a time on the recovery bridge. The idempotency key keeps a
     * retried submit from executing twice; the UI never infers success — the
     * operation has to report a terminal stage.
     */
    private fun submitRecovery(command: org.json.JSONObject, label: String? = null) {
        val bridge = recoveryBridge ?: return
        val kind = command.optString("command", "admin_recovery")
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "$kind-${label ?: refreshNetwork ?: "-"}-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    private fun settingsData(controller: FileBrowseController?): SettingsPageData {
        val preferences = getSharedPreferences(PREFERENCES, MODE_PRIVATE)
        val url = preferences.getString(KEY_API_URL, null)
        val hasToken = !preferences.getString(KEY_API_TOKEN, null).isNullOrBlank()
        return nodeApiSettingsData(
            url = url,
            hasToken = hasToken,
            connected = controller != null,
            onSave = title@{ url, token ->
                val validation = SettingsValidation.of(url, token)
                if (!validation.isValid) return@title validation
                preferences.edit()
                    .putString(KEY_API_URL, url.trim())
                    .putString(KEY_API_TOKEN, token.trim().ifEmpty { null })
                    .apply()
                bridgeScope.launch { recreate() }
                validation
            },
            onClear = {
                preferences.edit().remove(KEY_API_URL).remove(KEY_API_TOKEN).apply()
                bridgeScope.launch { recreate() }
            },
        )
    }

    private fun browseActions(
        controller: FileBrowseController?,
        requestUpload: () -> Unit,
        downloadFile: (DirectoryEntry) -> Unit,
    ): FileBrowseActions? =
        controller?.let {
            FileBrowseActions(
                selectTarget = { target -> it.open(target, "") },
                openEntry = { entry ->
                    if (entry.isDirectory) {
                        val current = it.state().value
                        it.open(current.selected ?: return@FileBrowseActions, entry.path)
                    }
                },
                up = it::up,
                loadMore = it::loadMore,
                refresh = it::loadTargets,
                requestUpload = requestUpload,
                downloadFile = downloadFile,
            )
        }

    /**
     * Starts a token download for one file entry. The saved document is picked
     * afterwards, so the token is held until the operator chooses a target.
     */
    private fun startDownload(entry: DirectoryEntry) {
        val current = browseController?.state()?.value ?: return
        val target = current.selected ?: return
        pendingDownloadName = FileTransferNaming.suggestFileName(entry.path)
        transferController?.download(
            TransferRequest(
                peer = target.peer,
                shareId = target.shareId,
                path = entry.path,
                networkId = target.networkId,
            ),
        )
    }

    /** Uploads the picked document into the directory the browser is showing. */
    private fun uploadPicked(uri: android.net.Uri) {
        val config = apiConfig ?: return
        val current = browseController?.state()?.value ?: return
        val target = current.selected ?: return
        val length = contentResolver.uploadLength(uri)
        if (length == null) {
            // Nothing to send: the provider hid the size, and guessing one would
            // corrupt the upload. Say so instead of looking like a dead button.
            transferController?.reportClientError("file.page.error.unsupported_detail")
            return
        }
        val name = contentResolver.displayName(uri)
        val path = FileTransferNaming.uploadTargetPath(current.path, name)
        if (path == null) {
            transferController?.reportClientError("file.page.error.invalid_filename_detail")
            return
        }
        transferController?.upload(
            TransferRequest(
                peer = target.peer,
                shareId = target.shareId,
                path = path,
                networkId = target.networkId,
            ),
            contentLength = length,
            open = { contentResolver.openUploadSource(uri) },
        )
        // `config` is only read through the controller's transport; keep the
        // validity check non-throwing so a stale config cannot crash the picker.
        if (!config.isValid) {
            transferController?.reportClientError("file.page.error.unsupported_detail")
        }
    }

    /**
     * Hot-applies a routing update. The form is only a request: once the node
     * answers `completed`, the snapshot is re-read so the applied summary the
     * page shows is always the node's, never the form's intent.
     */
    private fun updateRouting(mode: String, exitNode: String?, isExitNode: Boolean) {
        val bridge = routingBridge ?: return
        val networkName = selectedNetwork.value ?: return
        val command = RoutingSurface.updateCommand(networkName, mode, exitNode, isExitNode)
            ?: return
        bridge.submit(
            bridgeScope,
            CommandJson.envelope(
                command = command,
                requestId = nextRequestId(),
                idempotencyKey = "routing-update-$networkName-${UUID.randomUUID()}",
            ),
        )
    }

    private fun setLocalExit(enabled: Boolean) {
        val bridge = exitBridge ?: return
        bridge.submit(
            bridgeScope,
            CommandJson.envelope(
                command = ExitSurface.localExitCommand(enabled),
                requestId = nextRequestId(),
                idempotencyKey = "local-exit-$enabled-${UUID.randomUUID()}",
            ),
        )
    }

    private fun refreshBlacklist(networkName: String? = selectedNetwork.value) {
        val bridge = blacklistBridge ?: return
        val command = BlacklistSurface.statusCommand(networkName.orEmpty()) ?: return
        bridge.submit(
            bridgeScope,
            CommandJson.envelope(
                command = command,
                requestId = nextRequestId(),
                idempotencyKey = "blacklist-status-$networkName-${UUID.randomUUID()}",
            ),
        )
    }

    private fun banClient(clientId: String) {
        submitBlacklistMutation(BlacklistSurface.banCommand(selectedNetwork.value.orEmpty(), clientId))
    }

    private fun unbanClient(clientId: String) {
        submitBlacklistMutation(BlacklistSurface.unbanCommand(selectedNetwork.value.orEmpty(), clientId))
    }

    private fun submitBlacklistMutation(command: org.json.JSONObject?) {
        val bridge = blacklistBridge ?: return
        val body = CommandJson.envelope(
            command = command ?: return,
            requestId = nextRequestId(),
            idempotencyKey = "blacklist-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    /**
     * Approves several pending applications in one revision-checked batch. Any
     * request whose revision is missing is dropped before submission; a partial
     * batch would otherwise be refused wholesale by the Coordinator.
     */
    private fun batchApproveJoinRequests(requests: List<JoinRequestView>) {
        val bridge = memberBridge ?: return
        val networkId = memberNetworkId ?: return
        val reviews = requests.mapNotNull { request ->
            val revision = request.revision ?: return@mapNotNull null
            JoinReviewRequest(
                networkId = networkId,
                requestId = request.requestId,
                action = "approve",
                expectedRevision = revision,
            )
        }
        if (reviews.isEmpty()) return
        val command = JoinRequestSurface.batchReviewCommand(networkId, reviews) ?: return
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "join-batch-${networkId}-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    private fun refreshInvitations(networkId: String) {
        val bridge = memberBridge ?: return
        val command = InvitationSurface.listCommand(networkId) ?: return
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "invitation-list-$networkId-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    /** Creates one invitation with the default member/1-use/24h profile. */
    private fun createInvitation() {
        val bridge = memberBridge ?: return
        val networkId = memberNetworkId ?: return
        val command = InvitationSurface.createCommand(
            CreateInvitationRequest(networkId = networkId),
        ) ?: return
        issuedInvitationCode.value = null
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "invitation-create-$networkId-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    private fun revokeInvitation(invitation: InvitationView) {
        val bridge = memberBridge ?: return
        val networkId = memberNetworkId ?: return
        val revision = invitation.revision ?: return
        val command = InvitationSurface.revokeCommand(networkId, invitation.invitationId, revision)
            ?: return
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "invitation-revoke-${invitation.invitationId}-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    private fun refreshJoinRequests(networkId: String) {
        val bridge = memberBridge ?: return
        val body = CommandJson.envelope(
            command = JoinRequestSurface.listCommand(networkId),
            requestId = nextRequestId(),
            idempotencyKey = "join-list-$networkId-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    /**
     * Reviews one application with the revision the reviewer was shown. A stale
     * revision is refused by the Coordinator; the UI never re-sends with a
     * guessed value.
     */
    private fun reviewJoinRequest(action: String, request: JoinRequestView) {
        val bridge = memberBridge ?: return
        val networkId = memberNetworkId ?: return
        val revision = request.revision ?: return
        val command = JoinRequestSurface.reviewCommand(
            JoinReviewRequest(
                networkId = networkId,
                requestId = request.requestId,
                action = action,
                expectedRevision = revision,
            ),
        ) ?: return
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "join-$action-${request.requestId}-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    /**
     * Applies one member lifecycle change with the revision read from the
     * roster. A stale revision is rejected by the Coordinator, so the UI never
     * retries with a guessed value.
     */
    private fun changeMemberLifecycle(action: String, member: MemberView) {
        val bridge = memberBridge ?: return
        val networkId = memberNetworkId ?: return
        val revision = member.revision ?: return
        val command = MemberLifecycleSurface.command(
            MemberLifecycleRequest(
                networkId = networkId,
                targetNodeId = member.nodeId,
                action = action,
                role = member.role?.takeIf { it in MemberLifecycleSurface.ROLES } ?: "member",
                expectedRevision = revision,
            ),
        ) ?: return
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "member-$action-${member.nodeId}-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    /** Reads the member roster through its own command bridge. */
    private fun refreshMembers(networkId: String) {
        val bridge = memberBridge ?: return
        val body = CommandJson.envelope(
            command = MemberSurface.listCommand(networkId),
            requestId = nextRequestId(),
            idempotencyKey = "member-list-$networkId-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    private fun nextRequestId(): Long {
        val candidate = requestSequence.incrementAndGet()
        return if (candidate > 0) candidate else 1
    }

    override fun onDestroy() {
        pollJob?.cancel()
        eventJob?.cancel()
        bridgeScope.cancel()
        bridge = null
        super.onDestroy()
    }

    /**
     * A node URL and token are configuration, not defaults: they come from the
     * launching intent (tooling, tests) or the app's own preferences. Without
     * them the app shows the unavailable surface instead of probing a guessed
     * address.
     */
    private fun hasSavedToken(): Boolean =
        !getSharedPreferences(PREFERENCES, MODE_PRIVATE)
            .getString(KEY_API_TOKEN, null).isNullOrBlank()

    /** Persists the first-run address and token, then restarts the shell. */
    private fun saveApiConfig(url: String, token: String) {
        getSharedPreferences(PREFERENCES, MODE_PRIVATE).edit()
            .putString(KEY_API_URL, url)
            .putString(KEY_API_TOKEN, token.ifBlank { null })
            .apply()
        recreate()
    }

    private fun loadApiConfig(intent: Intent?): NodeApiConfig? {
        val preferences = getSharedPreferences(PREFERENCES, MODE_PRIVATE)
        val url = intent?.getStringExtra(EXTRA_API_URL)
            ?.takeIf { it.isNotBlank() }
            ?: preferences.getString(KEY_API_URL, null)
        if (url.isNullOrBlank()) return null
        val token = intent?.getStringExtra(EXTRA_API_TOKEN)
            ?.takeIf { it.isNotBlank() }
            ?: preferences.getString(KEY_API_TOKEN, null)
        return NodeApiConfig(baseUrl = url, token = token)
    }

    companion object {
        const val EXTRA_API_URL = "node_api_url"
        const val EXTRA_API_TOKEN = "node_api_token"
        private const val PREFERENCES = "fuckipv6.node"
        private const val KEY_API_URL = "node_api_url"
        private const val KEY_API_TOKEN = "node_api_token"
    }
}
