package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ExpandLess
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.Tune
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Collapsed container for identifiers, wire reason codes and revision counters.
 *
 * Product surfaces show state, impact and the action a person can take; the raw
 * protocol values stay one deliberate tap away so an operator can still compare
 * them with logs or audit records. Collapsed by default, because a first-time
 * reader should never have to parse `net_…` or `recovery_…` to understand what
 * the screen says.
 */
@Composable
fun TechnicalDetails(
    contracts: SharedContracts,
    locale: Locale,
    modifier: Modifier = Modifier,
    titleKey: String = "common.tech_details",
    hintKey: String? = "common.tech_details_hint",
    testTagPrefix: String = "tech-details",
    content: @Composable ColumnScope.() -> Unit,
) {
    var expanded by rememberSaveable(testTagPrefix) { mutableStateOf(false) }
    Surface(
        modifier = modifier
            .fillMaxWidth()
            .testTag(testTagPrefix),
        shape = MaterialTheme.shapes.medium,
        color = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.45f),
    ) {
        Column {
            Row(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("$testTagPrefix-toggle")
                    .clickable { expanded = !expanded }
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(
                    imageVector = Icons.Outlined.Tune,
                    contentDescription = null,
                    modifier = Modifier.size(16.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    text = contracts.i18n.translate(locale, titleKey),
                    style = MaterialTheme.typography.labelLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
                Icon(
                    imageVector = if (expanded) {
                        Icons.Outlined.ExpandLess
                    } else {
                        Icons.Outlined.ExpandMore
                    },
                    contentDescription = null,
                    modifier = Modifier.size(18.dp),
                    tint = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (expanded) {
                Column(
                    modifier = Modifier.padding(start = 14.dp, end = 14.dp, bottom = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (hintKey != null) {
                        Text(
                            text = contracts.i18n.translate(locale, hintKey),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    content()
                }
            }
        }
    }
}

/** Label/value pair for raw technical values; the value renders as a code string. */
@Composable
fun TechValue(
    label: String,
    value: String,
    modifier: Modifier = Modifier,
) {
    Column(modifier = modifier.fillMaxWidth()) {
        Text(
            text = label,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = value,
            style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
            overflow = TextOverflow.Ellipsis,
            maxLines = 2,
        )
    }
}

/**
 * Shortens an opaque identifier for a headline while keeping enough of both ends
 * to be recognisable next to a log line. The full value stays available in the
 * technical details block.
 */
fun abbreviateId(value: String?, head: Int = 10, tail: Int = 4): String? {
    val trimmed = value?.trim().orEmpty()
    if (trimmed.isEmpty()) return null
    if (trimmed.length <= head + tail + 1) return trimmed
    return trimmed.take(head) + "…" + trimmed.takeLast(tail)
}