package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.NetworkLabel
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Network switcher.
 *
 * A node can serve several networks; every surface must read the *selected*
 * one instead of silently using whichever came first. When the selected name is
 * no longer in the snapshot the caller falls back to the first entry rather than
 * showing stale data from a network that disappeared.
 */
object NetworkSelection {
    fun resolve(
        available: List<NetworkLabel>,
        selectedName: String?,
    ): NetworkLabel? = available.firstOrNull { it.networkName == selectedName }
        ?: available.firstOrNull()

    fun names(available: List<NetworkLabel>): List<String> = available.map { it.networkName }
}

@Composable
fun NetworkSelector(
    contracts: SharedContracts,
    locale: Locale,
    available: List<NetworkLabel>,
    selectedName: String?,
    onSelect: (String) -> Unit,
) {
    if (available.size <= 1) return
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("network-selector"),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(
            text = contracts.i18n.translate(locale, "common.current_network"),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            available.forEach { network ->
                FilterChip(
                    modifier = Modifier.testTag("network-option-${network.networkId}"),
                    selected = network.networkName == selectedName,
                    onClick = { onSelect(network.networkName) },
                    label = { Text(network.networkName) },
                )
            }
        }
    }
}
