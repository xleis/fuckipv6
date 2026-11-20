package cc.xlei.fuckipv6.android.contract

import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class I18nCatalogFallbackTest {
    private val catalog = I18nCatalog.parse(
        """
        {"schema_version":1,"default_locale":"en","messages":{
          "zh-CN":{"traffic.row.tun_rx":"TUN 接收"},
          "en":{"traffic.row.tun_rx":"TUN receive"}}}
        """.trimIndent(),
    )

    @Test
    fun missingKeysAreReportedInsteadOfRenderedAsIdentifiers() {
        val zh = Locale.SIMPLIFIED_CHINESE
        assertTrue(catalog.has(zh, "traffic.row.tun_rx"))
        assertFalse(catalog.has(zh, "activity.action.future_thing"))
        assertEquals("原始动作", catalog.translateOr(zh, "activity.action.future_thing", "原始动作"))
        assertEquals("TUN 接收", catalog.translateOr(zh, "traffic.row.tun_rx", "fallback"))
    }

    @Test
    fun englishFallsBackToTheDefaultLocaleCatalog() {
        val en = Locale.ENGLISH
        assertTrue(catalog.has(en, "traffic.row.tun_rx"))
        assertEquals("TUN receive", catalog.translateOr(en, "traffic.row.tun_rx", "fallback"))
    }
}
