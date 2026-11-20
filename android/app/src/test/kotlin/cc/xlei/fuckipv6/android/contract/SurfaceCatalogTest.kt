package cc.xlei.fuckipv6.android.contract

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.ui.browseErrorKey
import cc.xlei.fuckipv6.android.ui.downloadErrorKey
import cc.xlei.fuckipv6.android.ui.transferErrorKey
import cc.xlei.fuckipv6.android.ui.SurfaceStatus
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/**
 * Runtime key resolution for every string the Android surfaces compose.
 *
 * `check-android-i18n.py` covers literal keys; the status keys and the
 * `status.capability.*` keys are built at runtime, so they are asserted here
 * against the shipped catalog. A missing key would otherwise render a raw
 * identifier on screen.
 */
class SurfaceCatalogTest {
    private val catalog: Map<String, JSONObject> by lazy {
        val text = javaClass.classLoader!!
            .getResourceAsStream("i18n.json")!!
            .bufferedReader()
            .use { it.readText() }
        val messages = JSONObject(text).getJSONObject("messages")
        messages.keys().asSequence().associateWith { locale ->
            messages.getJSONObject(locale)
        }
    }

    private fun assertKey(key: String) {
        for (locale in catalog.keys) {
            val messages = catalog[locale]!!
            if (!messages.has(key) || messages.optString(key).isBlank()) {
                fail("$key is missing from $locale")
            }
        }
    }

    @Test
    fun everySurfaceStatusHasTitleAndDetailCopyInBothLocales() {
        for (status in SurfaceStatus.values()) {
            assertKey("state.${status.wireName}.title")
            assertKey("state.${status.wireName}.detail")
        }
    }

    @Test
    fun everyCapabilityStateKeyExists() {
        for (state in CAPABILITY_STATES) {
            assertKey("status.capability.$state")
        }
    }

    @Test
    fun everyBridgeFailureKeyResolves() {
        val failures = listOf(
            BridgeFailure.Unauthorized(401),
            BridgeFailure.HttpStatus(429),
            BridgeFailure.HttpStatus(503),
            BridgeFailure.Unreachable,
            BridgeFailure.Malformed,
            BridgeFailure.Misconfigured,
        )
        for (failure in failures) {
            assertKey(failure.uiStateKey)
            assertKey(failure.browseErrorKey())
            assertKey(failure.transferErrorKey())
            assertKey(failure.downloadErrorKey())
        }
    }

    @Test
    fun navigationPagesHaveLocalizedLabels() {
        val navigation = javaClass.classLoader!!
            .getResourceAsStream("ui-navigation.json")!!
            .bufferedReader()
            .use { JSONObject(it.readText()) }
        val modules = navigation.getJSONArray("modules")
        assertTrue("navigation contract has no modules", modules.length() > 0)
        for (index in 0 until modules.length()) {
            val module = modules.getJSONObject(index)
            assertKey(module.getString("label_key"))
            val pages = module.optJSONArray("page_ids") ?: continue
            for (pageIndex in 0 until pages.length()) {
                assertKey("page.${pages.getString(pageIndex)}")
            }
        }
    }

    private companion object {
        /** States `FilePageData` can project (see `FilePageData.SUPPORTED_STATES`). */
        val CAPABILITY_STATES = listOf(
            "disabled",
            "starting",
            "enabled",
            "degraded",
            "unavailable",
            "unsupported",
            "unknown",
        )
    }
}
