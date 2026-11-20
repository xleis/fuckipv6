package cc.xlei.fuckipv6.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsProjectionTest {
    @Test
    fun emptyUrlIsNotAnErrorUntilItIsSubmitted() {
        val untouched = SettingsValidation.of("", "")
        assertTrue(untouched.isValid)
        assertNull(untouched.configOrNull()?.baseUrl)

        val schemeMissing = SettingsValidation.of("127.0.0.1:6668", "")
        assertFalse(schemeMissing.isValid)
        assertEquals("settings.node_api.invalid_url", schemeMissing.errorKey)
        assertNull(schemeMissing.configOrNull())

        val wrongScheme = SettingsValidation.of("ftp://127.0.0.1", "")
        assertEquals("settings.node_api.invalid_url", wrongScheme.errorKey)

        val spaced = SettingsValidation.of("http://127.0.0.1 :1", "")
        assertEquals("settings.node_api.invalid_url", spaced.errorKey)
    }

    @Test
    fun validFormProducesTrimmedConfigAndHidesBlankToken() {
        val config = SettingsValidation.of(" http://10.0.2.2:6668/ ", "  token-1 ")
            .configOrNull()!!
        assertEquals("http://10.0.2.2:6668/", config.baseUrl)
        assertEquals("token-1", config.token)

        val withoutToken = SettingsValidation.of("http://127.0.0.1:6668", "   ").configOrNull()!!
        assertNull(withoutToken.token)
    }

    @Test
    fun formSubmitStoresTheValidationOutcome() {
        val invalid = SettingsFormState(url = "localhost").submit()
        assertEquals("settings.node_api.invalid_url", invalid.validation?.errorKey)

        val editingAgain = invalid.editing(url = "http://localhost:6668")
        assertNull(editingAgain.validation)

        val valid = editingAgain.submit()
        assertTrue(valid.validation?.isValid == true)
    }

    @Test
    fun pageDataCarriesOnlyWhatTheUiNeeds() {
        var saved: Pair<String, String>? = null
        var cleared = false
        val data = nodeApiSettingsData(
            url = "http://127.0.0.1:6668",
            hasToken = true,
            connected = true,
            onSave = { url, token ->
                saved = url to token
                SettingsValidation.of(url, token)
            },
            onClear = { cleared = true },
        )
        assertEquals("http://127.0.0.1:6668", data.currentUrl)
        assertTrue(data.hasToken)
        assertTrue(data.connected)
        data.onSave("http://10.0.2.2:6668", "")
        assertEquals("http://10.0.2.2:6668" to "", saved)
        data.onClear()
        assertTrue(cleared)
    }
}
