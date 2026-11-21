package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * First-run setup: the node address and token come from the operator, never
 * from a guessed default.
 *
 * The form validates locally before anything is saved, and the saved token is
 * never echoed back — a re-opened form shows only whether a token exists.
 */
@Composable
fun OnboardingPage(
    contracts: SharedContracts,
    locale: Locale,
    state: OnboardingState,
    savedUrl: String?,
    hasSavedToken: Boolean,
    onConnect: (url: String, token: String) -> Unit,
) {
    var url by remember(savedUrl) { mutableStateOf(savedUrl.orEmpty()) }
    var token by remember { mutableStateOf("") }
    var errorKey by remember { mutableStateOf<String?>(null) }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp)
            .testTag("onboarding"),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(
            modifier = Modifier.semantics { heading() },
            text = contracts.i18n.translate(locale, "settings.node_api.title"),
            style = MaterialTheme.typography.headlineSmall,
        )
        Text(
            modifier = Modifier.testTag("onboarding-detail"),
            text = contracts.i18n.translate(
                locale,
                state.messageKey ?: "onboarding.configure.detail",
            ),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        OutlinedTextField(
            modifier = Modifier
                .fillMaxWidth()
                .testTag("onboarding-url"),
            value = url,
            onValueChange = { url = it },
            label = { Text(contracts.i18n.translate(locale, "settings.node_api.url_label")) },
            singleLine = true,
        )
        OutlinedTextField(
            modifier = Modifier
                .fillMaxWidth()
                .testTag("onboarding-token"),
            value = token,
            onValueChange = { token = it },
            label = {
                Text(
                    contracts.i18n.translate(
                        locale,
                        if (hasSavedToken) "settings.node_api.token_hint" else "settings.node_api.token_label",
                    ),
                )
            },
            singleLine = true,
        )
        errorKey?.let { key ->
            Text(
                modifier = Modifier.testTag("onboarding-error"),
                text = contracts.i18n.translate(locale, key),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.error,
            )
        }
        Button(
            modifier = Modifier.testTag("onboarding-connect"),
            onClick = {
                val validation = SettingsValidation.of(url, token)
                errorKey = validation.errorKey
                if (validation.isValid) {
                    onConnect(validation.url.trim(), validation.token.trim())
                }
            },
        ) {
            Text(contracts.i18n.translate(locale, "onboarding.action.connect"))
        }
        Text(
            text = contracts.i18n.translate(locale, "onboarding.token_note"),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
