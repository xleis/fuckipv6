package cc.xlei.fuckipv6.android.contract

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Locale

class SharedContractsTest {
    private fun resource(name: String): String =
        requireNotNull(javaClass.getResource("/$name")) { "missing test resource $name" }.readText()

    @Test
    fun sharedNavigationDefinesFiveModulesAndFourteenUniquePages() {
        val navigation = NavigationContract.parse(resource("ui-navigation.json"))

        assertEquals(1, navigation.schemaVersion)
        assertEquals(
            listOf("overview", "nodes", "networks", "activity", "settings"),
            navigation.modules.map { it.id },
        )
        assertEquals(
            mapOf(
                "overview" to listOf("overview"),
                "nodes" to listOf("peers", "node_runtime", "routing", "identity"),
                "networks" to listOf("networks", "topology", "exit_node", "relay"),
                "activity" to listOf("traffic", "activity", "logs", "files"),
                "settings" to listOf("settings"),
            ),
            navigation.modules.associate { it.id to it.pageIds },
        )
        assertEquals(14, navigation.modules.sumOf { it.pageIds.size })
        assertEquals(14, navigation.modules.flatMap { it.pageIds }.toSet().size)
    }

    @Test
    fun sharedTranslationsCoverBothLocalesAndReplaceParameters() {
        val catalog = I18nCatalog.parse(resource("i18n.json"))

        assertEquals("en", catalog.defaultLocale)
        assertEquals("概览", catalog.translate(Locale.SIMPLIFIED_CHINESE, "nav.overview"))
        assertEquals("Overview", catalog.translate(Locale.ENGLISH, "nav.overview"))
        assertEquals(
            "审计队列已淘汰 7 条旧事件。",
            catalog.translate(
                Locale.SIMPLIFIED_CHINESE,
                "relay.governance.dropped",
                mapOf("dropped" to "7"),
            ),
        )
        assertEquals(
            "The audit queue evicted 7 older events.",
            catalog.translate(
                Locale.ENGLISH,
                "relay.governance.dropped",
                mapOf("dropped" to "7"),
            ),
        )
    }

    @Test
    fun sharedNavigationRejectsDuplicatePageOwnership() {
        val invalid = """
            {
              "schema_version": 1,
              "modules": [
                {"id":"a","label_key":"a","default_page_id":"one","page_ids":["one","two"]},
                {"id":"b","label_key":"b","default_page_id":"three","page_ids":["three","four"]},
                {"id":"c","label_key":"c","default_page_id":"five","page_ids":["five"]},
                {"id":"d","label_key":"d","default_page_id":"six","page_ids":["six"]},
                {"id":"e","label_key":"e","default_page_id":"seven","page_ids":["seven","eight","nine","ten","eleven","twelve","one"]}
              ]
            }
        """.trimIndent()

        val error = runCatching { NavigationContract.parse(invalid) }.exceptionOrNull()
        assertTrue(error is IllegalArgumentException)
        val message = error?.message.orEmpty()
        assertTrue(message.contains("more than one navigation module"))
    }
}
