package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Node connection settings.
 *
 * This is the only way to configure the app without adb extras. Saving is
 * explicit: the form validates the address with the same rule the bridge uses,
 * and only a valid form is handed to the callback that persists it.
 */
@Composable
fun SettingsPageContent(
    contracts: SharedContracts,
    locale: Locale,
    data: SettingsPageData,
) {
    var form by remember(data.currentUrl, data.hasToken) {
        mutableStateOf(SettingsFormState(url = data.currentUrl))
    }
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("settings-node-api-card"),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "settings.node_api.title"),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = contracts.i18n.translate(locale, "settings.node_api.detail"),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("settings-node-api-state"),
                text = contracts.i18n.translate(
                    locale,
                    if (data.connected) "common.live" else "state.control_unavailable.title",
                ),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            OutlinedTextField(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("settings-node-api-url"),
                value = form.url,
                onValueChange = { form = form.editing(url = it) },
                label = { Text(contracts.i18n.translate(locale, "settings.node_api.url_label")) },
                singleLine = true,
            )
            OutlinedTextField(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("settings-node-api-token"),
                value = form.token,
                onValueChange = { form = form.editing(token = it) },
                label = { Text(contracts.i18n.translate(locale, "settings.node_api.token_label")) },
                visualTransformation = PasswordVisualTransformation(),
                singleLine = true,
            )
            Text(
                text = contracts.i18n.translate(locale, "settings.node_api.token_hint"),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            form.validation?.errorKey?.let { key ->
                Text(
                    modifier = Modifier.testTag("settings-node-api-error"),
                    text = contracts.i18n.translate(locale, key),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Button(
                    modifier = Modifier.testTag("settings-node-api-save"),
                    onClick = {
                        val submitted = form.submit()
                        form = submitted
                        submitted.validation?.takeIf { it.isValid }?.let { valid ->
                            data.onSave(valid.url, valid.token)
                        }
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "settings.node_api.save"))
                }
                TextButton(
                    modifier = Modifier.testTag("settings-node-api-clear"),
                    onClick = {
                        form = SettingsFormState()
                        data.onClear()
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "settings.node_api.clear"))
                }
            }
        }
    }
}