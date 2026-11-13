package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FuckIpv6App(contracts: SharedContracts) {
    val systemLocale = LocalConfiguration.current.locales[0] ?: Locale.getDefault()
    val initialModule = contracts.navigation.modules.first()
    var selectedModuleId by rememberSaveable { mutableStateOf(initialModule.id) }
    var selectedPageId by rememberSaveable { mutableStateOf(initialModule.defaultPageId) }
    val selectedModule = contracts.navigation.modules.first { it.id == selectedModuleId }
    val unavailable = SurfaceState(SurfaceStatus.CONTROL_UNAVAILABLE)

    Scaffold(
        topBar = {
            CenterAlignedTopAppBar(
                title = {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("FuckIPv6", style = MaterialTheme.typography.titleMedium)
                        Text(
                            text = contracts.i18n.translate(systemLocale, selectedModule.labelKey),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
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
                state = unavailable,
                modifier = Modifier.weight(1f),
            )
        }
    }
}

@Composable
private fun ModuleIcon(moduleId: String) {
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
        SurfaceNotice(state = state, locale = locale, contracts = contracts)
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
    Box(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        contentAlignment = Alignment.Center,
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                text = "共享 UI 契约加载失败 / Shared UI contract failed",
                style = MaterialTheme.typography.titleLarge,
            )
            Text(
                text = message ?: "contract_violation",
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}
