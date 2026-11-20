package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.List
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.Card
import androidx.compose.material3.CenterAlignedTopAppBar
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.PrimaryScrollableTabRow
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLocale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.R
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FuckIpv6App(
    contracts: SharedContracts,
    relay: RelayPageData? = null,
    coordinator: CoordinatorPageData? = null,
    adminRecovery: AdminRecoveryPageData? = null,
    files: FilePageData? = null,
    filesFailureKey: String? = null,
    settings: SettingsPageData? = null,
    fileBrowse: FileWorkspaceState? = null,
    fileBrowseActions: FileBrowseActions? = null,
    transfers: FileTransferState? = null,
    members: cc.xlei.fuckipv6.android.bridge.MemberDirectoryView? = null,
    onRefreshMembers: (() -> Unit)? = null,
    onMemberLifecycleAction: ((String, cc.xlei.fuckipv6.android.bridge.MemberView) -> Unit)? = null,
    joinRequests: List<cc.xlei.fuckipv6.android.bridge.JoinRequestView>? = null,
    onJoinReview: ((String, cc.xlei.fuckipv6.android.bridge.JoinRequestView) -> Unit)? = null,
) {
    val systemLocale = LocalLocale.current.platformLocale
    val initialModule = contracts.navigation.modules.first()
    var selectedModuleId by rememberSaveable { mutableStateOf(initialModule.id) }
    var selectedPageId by rememberSaveable { mutableStateOf(initialModule.defaultPageId) }
    val selectedModule = contracts.navigation.modules.first { it.id == selectedModuleId }
    val notOnThisDevice = SurfaceState(SurfaceStatus.NOT_AVAILABLE_ON_DEVICE)

    Scaffold(
        topBar = {
            CenterAlignedTopAppBar(
                title = {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        BrandMark()
                        Column(horizontalAlignment = Alignment.Start) {
                            Text("FuckIPv6", style = MaterialTheme.typography.titleMedium)
                            Text(
                                text = contracts.i18n.translate(
                                    systemLocale,
                                    selectedModule.labelKey,
                                ),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                },
            )
        },
        bottomBar = {
            NavigationBar {
                contracts.navigation.modules.forEach { module ->
                    val label = contracts.i18n.translate(systemLocale, module.labelKey)
                    NavigationBarItem(
                        modifier = Modifier
                            .heightIn(min = 48.dp)
                            .testTag("module-${module.id}"),
                        selected = module.id == selectedModuleId,
                        onClick = {
                            selectedModuleId = module.id
                            selectedPageId = module.defaultPageId
                        },
                        icon = { ModuleIcon(module.id) },
                        label = { Text(label) },
                    )
                }
            }
        },
    ) { innerPadding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(innerPadding),
        ) {
            if (selectedModule.pageIds.size > 1) {
                PrimaryScrollableTabRow(selectedTabIndex = selectedModule.pageIds.indexOf(selectedPageId)) {
                    selectedModule.pageIds.forEach { pageId ->
                        Tab(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("page-tab-$pageId"),
                            selected = pageId == selectedPageId,
                            onClick = { selectedPageId = pageId },
                            text = {
                                Text(
                                    contracts.i18n.translate(
                                        systemLocale,
                                        "page.$pageId",
                                    ),
                                )
                            },
                        )
                    }
                }
            }
            PageContent(
                contracts = contracts,
                locale = systemLocale,
                pageId = selectedPageId,
                state = notOnThisDevice,
                modifier = Modifier.weight(1f),
                relay = relay,
                coordinator = coordinator,
                adminRecovery = adminRecovery,
                files = files,
                filesFailureKey = filesFailureKey,
                settings = settings,
                fileBrowse = fileBrowse,
                fileBrowseActions = fileBrowseActions,
                transfers = transfers,
                members = members,
                onRefreshMembers = onRefreshMembers,
                onMemberLifecycleAction = onMemberLifecycleAction,
                joinRequests = joinRequests,
                onJoinReview = onJoinReview,
            )
        }
    }
}

/** Brand glyph: the same open-ring mark the launcher icon uses, tinted by theme. */
@Composable
private fun BrandMark() {
    Icon(
        painter = painterResource(R.drawable.ic_brand_mark),
        contentDescription = null,
        modifier = Modifier.size(22.dp),
        tint = MaterialTheme.colorScheme.primary,
    )
}

@Composable
private fun ModuleIcon(moduleId: String) {
    // One glyph per module; the navigation bar tints the active destination, so
    // the icon set stays quiet and only the brand mark carries colour.
    val image = when (moduleId) {
        "overview" -> Icons.Outlined.Home
        "nodes" -> Icons.Outlined.Share
        "networks" -> Icons.AutoMirrored.Outlined.List
        "activity" -> Icons.Outlined.Info
        else -> Icons.Outlined.Settings
    }
    Icon(imageVector = image, contentDescription = null)
}

@Composable
fun PageContent(
    contracts: SharedContracts,
    locale: Locale,
    pageId: String,
    state: SurfaceState,
    modifier: Modifier = Modifier,
    relay: RelayPageData? = null,
    coordinator: CoordinatorPageData? = null,
    adminRecovery: AdminRecoveryPageData? = null,
    files: FilePageData? = null,
    filesFailureKey: String? = null,
    settings: SettingsPageData? = null,
    fileBrowse: FileWorkspaceState? = null,
    fileBrowseActions: FileBrowseActions? = null,
    transfers: FileTransferState? = null,
    members: cc.xlei.fuckipv6.android.bridge.MemberDirectoryView? = null,
    onRefreshMembers: (() -> Unit)? = null,
    onMemberLifecycleAction: ((String, cc.xlei.fuckipv6.android.bridge.MemberView) -> Unit)? = null,
    joinRequests: List<cc.xlei.fuckipv6.android.bridge.JoinRequestView>? = null,
    onJoinReview: ((String, cc.xlei.fuckipv6.android.bridge.JoinRequestView) -> Unit)? = null,
) {
    Column(
        modifier = modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(PaddingValues(horizontal = 20.dp, vertical = 18.dp)),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(
            modifier = Modifier.semantics { heading() },
            text = contracts.i18n.translate(locale, "page.$pageId"),
            style = MaterialTheme.typography.headlineSmall,
        )
        when {
            pageId == "node_runtime" && (adminRecovery != null || coordinator != null) ->
                Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    adminRecovery?.let {
                        AdminRecoveryPageContent(
                            contracts = contracts,
                            locale = locale,
                            data = it,
                        )
                    }
                    coordinator?.let {
                        CoordinatorPageContent(
                            contracts = contracts,
                            locale = locale,
                            data = it,
                        )
                    }
                    if (onRefreshMembers != null) {
                        MemberListContent(
                            contracts = contracts,
                            locale = locale,
                            directory = members,
                            onRefresh = onRefreshMembers,
                            onLifecycleAction = onMemberLifecycleAction,
                        )
                        JoinRequestListContent(
                            contracts = contracts,
                            locale = locale,
                            requests = joinRequests,
                            onReview = onJoinReview,
                        )
                    }
                }
            pageId == "settings" && settings != null ->
                SettingsPageContent(contracts = contracts, locale = locale, data = settings)
            pageId == "files" && fileBrowse != null && fileBrowseActions != null ->
                Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    files?.let { FilePageContent(contracts = contracts, locale = locale, data = it) }
                    FileBrowseContent(
                        contracts = contracts,
                        locale = locale,
                        state = fileBrowse,
                        onSelectTarget = fileBrowseActions.selectTarget,
                        onOpenEntry = fileBrowseActions.openEntry,
                        onUp = fileBrowseActions.up,
                        onLoadMore = fileBrowseActions.loadMore,
                        onRefresh = fileBrowseActions.refresh,
                        actions = fileBrowseActions,
                    )
                    transfers?.let {
                        FileTransferSummary(
                            contracts = contracts,
                            locale = locale,
                            state = it,
                        )
                    }
                }
            pageId == "files" && files != null ->
                FilePageContent(contracts = contracts, locale = locale, data = files)
            pageId == "files" && filesFailureKey != null ->
                SurfaceNotice(
                    state = SurfaceState(
                        failureStatus(filesFailureKey),
                        detailKeyOverride = filesFailureKey,
                    ),
                    locale = locale,
                    contracts = contracts,
                )
            pageId == "relay" && relay != null && relay.backendAvailable ->
                RelayPageContent(contracts = contracts, locale = locale, data = relay)
            else -> SurfaceNotice(state = state, locale = locale, contracts = contracts)
        }
    }
}

@Composable
private fun SurfaceNotice(
    state: SurfaceState,
    locale: Locale,
    contracts: SharedContracts,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, state.titleKey),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = contracts.i18n.translate(locale, state.detailKey),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
fun ContractFailureScreen(message: String?) {
    var showRawMessage by remember { mutableStateOf(false) }
    Box(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(
                text = "界面无法加载 / Interface unavailable",
                style = MaterialTheme.typography.titleLarge,
            )
            Text(
                text = "共享界面配置未能加载，导航与文案因此不可用。请重新启动应用；若问题持续，" +
                    "可在技术详情中查看原因。",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                modifier = Modifier
                    .testTag("contract-failure-toggle")
                    .clickable { showRawMessage = !showRawMessage },
                text = if (showRawMessage) "隐藏技术详情" else "技术详情",
                color = MaterialTheme.colorScheme.primary,
                style = MaterialTheme.typography.labelLarge,
            )
            if (showRawMessage) {
                Text(
                    modifier = Modifier.testTag("contract-failure-detail"),
                    text = message ?: "contract_violation",
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    style = MaterialTheme.typography.bodySmall
                        .copy(fontFamily = FontFamily.Monospace),
                )
            }
        }
    }
}
