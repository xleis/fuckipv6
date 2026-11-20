package cc.xlei.fuckipv6.android.contract

import android.content.res.AssetManager
import org.json.JSONObject
import java.util.Locale

data class NavigationModule(
    val id: String,
    val labelKey: String,
    val defaultPageId: String,
    val pageIds: List<String>,
)

data class NavigationContract(val schemaVersion: Int, val modules: List<NavigationModule>) {
    companion object {
        fun parse(json: String): NavigationContract {
            val root = JSONObject(json)
            val schemaVersion = root.getInt("schema_version")
            require(schemaVersion == 1) { "unsupported navigation schema: $schemaVersion" }
            val rawModules = root.getJSONArray("modules")
            require(rawModules.length() == 5) { "navigation must contain exactly five modules" }

            val moduleIds = mutableSetOf<String>()
            val pageIds = mutableSetOf<String>()
            val modules = buildList {
                for (index in 0 until rawModules.length()) {
                    val raw = rawModules.getJSONObject(index)
                    val id = raw.getString("id")
                    val labelKey = raw.getString("label_key")
                    val defaultPageId = raw.getString("default_page_id")
                    val rawPageIds = raw.getJSONArray("page_ids")
                    require(id.isNotBlank() && labelKey.isNotBlank()) {
                        "navigation module $index is missing id or label"
                    }
                    require(moduleIds.add(id)) { "duplicate navigation module: $id" }
                    require(rawPageIds.length() > 0) { "navigation module $id has no pages" }
                    val pages = buildList {
                        for (pageIndex in 0 until rawPageIds.length()) {
                            val pageId = rawPageIds.getString(pageIndex)
                            require(pageId.isNotBlank()) { "navigation module $id has a blank page id" }
                            require(pageIds.add(pageId)) {
                                "page $pageId belongs to more than one navigation module"
                            }
                            add(pageId)
                        }
                    }
                    require(defaultPageId in pages) {
                        "navigation module $id default page $defaultPageId is not in page_ids"
                    }
                    add(NavigationModule(id, labelKey, defaultPageId, pages))
                }
            }
            require(pageIds.size == 14) { "navigation must contain exactly fourteen pages" }
            return NavigationContract(schemaVersion, modules)
        }
    }
}

class I18nCatalog internal constructor(
    val schemaVersion: Int,
    val defaultLocale: String,
    private val messages: Map<String, Map<String, String>>,
) {
    /**
     * Whether the resolved locale actually carries [key]. Callers that build a
     * key from server-provided identifiers (action names, outcome codes) must
     * check first: `translate` falls back to the key itself, which would put a
     * raw identifier on screen.
     */
    fun has(locale: Locale, key: String): Boolean {
        val localeKey = when {
            locale.language.equals("zh", ignoreCase = true) -> "zh-CN"
            else -> defaultLocale
        }
        return messages[localeKey]?.containsKey(key) == true
    }

    /** Translated label for [key], or [fallback] when the catalog lacks it. */
    fun translateOr(locale: Locale, key: String, fallback: String): String =
        if (has(locale, key)) translate(locale, key) else fallback

    fun translate(locale: Locale, key: String, params: Map<String, String> = emptyMap()): String {
        val localeKey = when {
            locale.language.equals("zh", ignoreCase = true) -> "zh-CN"
            else -> defaultLocale
        }
        val catalog = messages[localeKey] ?: messages[defaultLocale] ?: emptyMap()
        var value = catalog[key] ?: key
        for ((name, replacement) in params) {
            value = value.replace("{$name}", replacement)
        }
        return value
    }

    companion object {
        fun parse(json: String): I18nCatalog {
            val root = JSONObject(json)
            val schemaVersion = root.getInt("schema_version")
            require(schemaVersion == 1) { "unsupported i18n schema: $schemaVersion" }
            val defaultLocale = root.getString("default_locale")
            val rawMessages = root.getJSONObject("messages")
            val messages = rawMessages.keys().asSequence().associateWith { locale ->
                val rawCatalog = rawMessages.getJSONObject(locale)
                rawCatalog.keys().asSequence().associateWith(rawCatalog::getString)
            }
            require(messages.containsKey(defaultLocale)) {
                "i18n default locale $defaultLocale is missing"
            }
            return I18nCatalog(schemaVersion, defaultLocale, messages)
        }
    }
}

data class SharedContracts(val navigation: NavigationContract, val i18n: I18nCatalog)

object SharedContractLoader {
    fun load(assets: AssetManager): SharedContracts {
        val navigation = NavigationContract.parse(assets.open("ui-navigation.json").bufferedReader().use { it.readText() })
        val i18n = I18nCatalog.parse(assets.open("i18n.json").bufferedReader().use { it.readText() })
        return SharedContracts(navigation, i18n)
    }
}
