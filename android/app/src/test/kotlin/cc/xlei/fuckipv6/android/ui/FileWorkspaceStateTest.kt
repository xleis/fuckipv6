package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.DirectoryEntry
import cc.xlei.fuckipv6.android.bridge.DirectoryPage
import cc.xlei.fuckipv6.android.bridge.FileTarget
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FileWorkspaceStateTest {
    private val target = FileTarget(
        peer = "peer-a",
        networkId = "net-1",
        networkName = "家庭网络",
        shareId = "share-1",
    )

    private fun entry(path: String) = DirectoryEntry(path, false, 10, null)

    private fun page(vararg paths: String, cursor: String? = null, truncated: Boolean = false) =
        DirectoryPage(paths.map(::entry), cursor, truncated)

    @Test
    fun portablePathsJoinAndClimbWithoutEscaping() {
        assertEquals("docs", SharedPathNav.join("", "docs"))
        assertEquals("docs/a.md", SharedPathNav.join("docs", "a.md"))
        assertEquals("docs", SharedPathNav.parent("docs/a.md"))
        assertEquals("", SharedPathNav.parent("docs"))
        assertTrue(SharedPathNav.isRoot(""))
        assertTrue(SharedPathNav.isRoot(SharedPathNav.parent("docs")))

        assertTrue(SharedPathNav.isValid(""))
        assertTrue(SharedPathNav.isValid("docs/a.md"))
        assertFalse(SharedPathNav.isValid("/etc/passwd"))
        assertFalse(SharedPathNav.isValid("docs/"))
        assertFalse(SharedPathNav.isValid("docs//a"))
        assertFalse(SharedPathNav.isValid("../a"))
        assertFalse(SharedPathNav.isValid("docs\\a"))
        assertFalse(SharedPathNav.isValid("docs/./a"))
    }

    @Test
    fun loadingADirectoryResetsEntriesAndStartsANewGeneration() {
        val state = FileWorkspaceState()
            .applying(page("old"), generation = 0)
            .loadingDirectory(target, "docs")
        assertEquals(target, state.selected)
        assertEquals("docs", state.path)
        assertTrue(state.entries.isEmpty())
        assertTrue(state.loading)
        assertEquals(1L, state.generation)
        assertTrue(state.canGoUp)
    }

    @Test
    fun lateResponsesFromAnOlderDirectoryAreIgnored() {
        val state = FileWorkspaceState().loadingDirectory(target, "docs")
        val stale = state.applying(page("late.md"), generation = state.generation - 1)
        assertEquals(state, stale)
        assertTrue(stale.entries.isEmpty())

        val fresh = state.applying(page("fresh.md"), generation = state.generation)
        assertEquals(listOf("fresh.md"), fresh.entries.map { it.path })
        assertFalse(fresh.loading)
    }

    @Test
    fun loadingMoreAppendsAndKeepsTheOpaqueCursor() {
        val first = FileWorkspaceState()
            .loadingDirectory(target, "")
            .applying(page("a", "b", cursor = "c-2", truncated = true), generation = 1)
        assertTrue(first.canLoadMore)
        assertEquals("c-2", first.nextCursor)

        val more = first.loadingMore()
        assertTrue(more.appending)
        val appended = more.applying(page("c", cursor = null), generation = 1)
        assertEquals(listOf("a", "b", "c"), appended.entries.map { it.path })
        assertNull(appended.nextCursor)
        assertFalse(appended.canLoadMore)

        // Without a next page, loadingMore is a no-op rather than a request.
        assertEquals(appended, appended.loadingMore())
    }

    @Test
    fun failuresUseExistingSurfaceCopyAndKeepThePage() {
        val loaded = FileWorkspaceState()
            .loadingDirectory(target, "docs")
            .applying(page("a"), generation = 1)
        val failed = loaded.loadingMore().failing(BridgeFailure.HttpStatus(429), generation = 1)
        assertEquals("file.page.error.rate_limited_detail", failed.errorKey)
        assertEquals(listOf("a"), failed.entries.map { it.path })
        assertFalse(failed.loading)

        assertEquals(
            "file.page.error.permission_detail",
            FileWorkspaceState().failing(BridgeFailure.Unauthorized(403), generation = 0).errorKey,
        )
        assertEquals(
            "file.page.error.unsupported_detail",
            FileWorkspaceState().failing(BridgeFailure.Misconfigured, generation = 0).errorKey,
        )
        assertEquals(
            "file.page.error.generic",
            FileWorkspaceState().failing(BridgeFailure.Malformed, generation = 0).errorKey,
        )
        assertEquals(
            "file.page.error.unavailable_detail",
            FileWorkspaceState().failing(BridgeFailure.Unreachable, generation = 0).errorKey,
        )
    }

    @Test
    fun targetLoadingKeepsTheFormUsable() {
        val failed = FileWorkspaceState().targetsFailed(BridgeFailure.Unreachable)
        assertEquals("file.page.error.unavailable_detail", failed.errorKey)
        assertTrue(failed.targets.isEmpty())

        val loaded = failed.targetsLoaded(listOf(target))
        assertEquals(1, loaded.targets.size)
        assertNull(loaded.errorKey)
    }
}
