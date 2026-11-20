package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FileBrowseTest {
    private val config = NodeApiConfig("http://127.0.0.1:6668")

    @Test
    fun targetsKeepPeerNetworkAndShareIdentity() {
        val targets = FileBrowseJson.parseTargets(
            """
            [
              {"peer":"peer-a","network_id":"net-1","network_name":"家庭网络","share_id":"share-1"},
              {"peer":"peer-b","network_id":"net-2","network_name":"工作网络","share_id":"share-2"},
              {"peer":"","network_id":"net-3","network_name":"x","share_id":"share-3"},
              {"peer":"peer-c","network_id":"net-4","network_name":"x"}
            ]
            """.trimIndent(),
        )
        assertEquals(2, targets.size)
        assertEquals("peer-a", targets[0].peer)
        assertEquals("net-1", targets[0].networkId)
        assertEquals("家庭网络", targets[0].networkName)
        assertEquals("share-1", targets[0].shareId)
    }

    @Test
    fun directoryParsesPaginationAndEntryKinds() {
        val page = FileBrowseJson.parseDirectory(
            """
            {
              "entries": [
                {"path":"docs","is_directory":true,"size":0},
                {"path":"docs/readme.md","is_directory":false,"size":2048,"modified_at_unix_ms":1760000000000}
              ],
              "next_cursor": "cursor-2",
              "truncated": true
            }
            """.trimIndent(),
        )
        assertEquals(2, page.entries.size)
        assertTrue(page.entries[0].isDirectory)
        assertEquals("docs/readme.md", page.entries[1].path)
        assertEquals(2048L, page.entries[1].size)
        assertEquals(1760000000000L, page.entries[1].modifiedAtUnixMs)
        assertEquals("cursor-2", page.nextCursor)
        assertTrue(page.truncated)

        val minimal = FileBrowseJson.parseDirectory("""{"entries":[]}""")
        assertTrue(minimal.entries.isEmpty())
        assertNull(minimal.nextCursor)
        assertFalse(minimal.truncated)
    }

    @Test
    fun malformedPayloadsAreRejected() {
        assertTrue(runCatching { FileBrowseJson.parseTargets("not json") }.isFailure)
        assertTrue(runCatching { FileBrowseJson.parseDirectory("[]") }.isFailure)
        assertTrue(runCatching { FileBrowseJson.parseDirectory("""{"entries":"nope"}""") }.isFailure)
    }

    @Test
    fun directoryUrlEncodesEveryQueryField() {
        val url = FileBrowseUrls.directory(
            config,
            BrowseRequest(
                peer = "peer a",
                shareId = "share/1",
                networkId = "net-1",
                path = "文档/报表.md",
                limit = 50,
                cursor = "cursor 2",
            ),
        )
        assertTrue(url.startsWith("http://127.0.0.1:6668/api/v1/file/directory?"))
        assertTrue(url.contains("peer=peer+a"))
        assertTrue(url.contains("share_id=share%2F1"))
        assertTrue(url.contains("network_id=net-1"))
        assertTrue(url.contains("path=%E6%96%87%E6%A1%A3%2F%E6%8A%A5%E8%A1%A8.md"))
        assertTrue(url.contains("limit=50"))
        assertTrue(url.contains("cursor=cursor+2"))

        val minimal = FileBrowseUrls.directory(config, BrowseRequest(peer = "p", shareId = "s"))
        assertFalse(minimal.contains("network_id="))
        assertFalse(minimal.contains("cursor="))
        assertTrue(minimal.contains("path=&"))
        assertEquals(
            "http://127.0.0.1:6668/api/v1/file/targets",
            FileBrowseUrls.targets(config),
        )
        assertEquals(
            "http://127.0.0.1:6668/api/v1/file/targets",
            FileBrowseUrls.targets(NodeApiConfig("http://127.0.0.1:6668/")),
        )
    }

    @Test
    fun missingConfigurationFailsClosedWithoutARequest() {
        val result = FileBrowseClient(NodeApiConfig("127.0.0.1:6668")).targets()
        assertTrue(result is BrowseResult.Failed)
        assertEquals(BridgeFailure.Misconfigured, (result as BrowseResult.Failed).failure)
    }
}
