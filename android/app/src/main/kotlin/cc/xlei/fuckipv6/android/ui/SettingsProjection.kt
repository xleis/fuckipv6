package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.NodeApiConfig

/** Outcome of validating the node-connection form. */
data class SettingsValidation(
    val url: String,
    val token: String,
    val errorKey: String?,
) {
    val isValid: Boolean
        get() = errorKey == null

    /** A blank address means "nothing configured", never a client for "". */
    fun configOrNull(): NodeApiConfig? =
        if (isValid && url.isNotBlank()) {
            NodeApiConfig(baseUrl = url.trim(), token = token.trim().ifEmpty { null })
        } else {
            null
        }

    companion object {
        /**
         * Validates the form before anything is persisted. The message is a
         * shared i18n key; the raw failure codes from [NodeApiConfig] stay
         * internal so the UI never renders an unlocalised string.
         */
        fun of(url: String, token: String): SettingsValidation {
            val trimmed = url.trim()
            val errorKey = when {
                trimmed.isEmpty() -> null
                NodeApiConfig(trimmed).validationError() != null -> "settings.node_api.invalid_url"
                else -> null
            }
            return SettingsValidation(url = trimmed, token = token.trim(), errorKey = errorKey)
        }
    }
}

/** What the settings page may do with the current connection. */
data class SettingsPageData(
    val currentUrl: String,
    val hasToken: Boolean,
    val connected: Boolean,
    val onSave: (String, String) -> SettingsValidation,
    val onClear: () -> Unit,
)

/** Values the page holds while the operator edits them. */
data class SettingsFormState(
    val url: String = "",
    val token: String = "",
    val validation: SettingsValidation? = null,
) {
    fun editing(url: String = this.url, token: String = this.token): SettingsFormState =
        copy(url = url, token = token, validation = null)

    fun submit(): SettingsFormState {
        val result = SettingsValidation.of(url, token)
        return copy(validation = result)
    }
}

internal fun nodeApiSettingsData(
    url: String?,
    hasToken: Boolean,
    connected: Boolean,
    onSave: (String, String) -> SettingsValidation,
    onClear: () -> Unit,
): SettingsPageData = SettingsPageData(
    currentUrl = url.orEmpty(),
    hasToken = hasToken,
    connected = connected,
    onSave = onSave,
    onClear = onClear,
)