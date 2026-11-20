package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.BrowseRequest
import cc.xlei.fuckipv6.android.bridge.BrowseResult
import cc.xlei.fuckipv6.android.bridge.BrowseSource
import cc.xlei.fuckipv6.android.bridge.DirectoryEntry
import cc.xlei.fuckipv6.android.bridge.DirectoryPage
import cc.xlei.fuckipv6.android.bridge.FileTarget
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FileBrowseControllerTest {
    private val target = FileTarget("peer-a", "net-1", "家庭网络", "share-1")

    private class FakeSource : BrowseSource {
        var targetsResult: BrowseResult<List<FileTarget>> = BrowseResult.Ok(emptyList())
        val requests = mutableListOf<BrowseRequest>()
        var directoryResult: BrowseResult<DirectoryPage> =
            BrowseResult.Ok(DirectoryPage(emptyList(), null, false))

        override fun targets() = targetsResult

        override fun directory(request: BrowseRequest): BrowseResult<DirectoryPage> {
            requests += request
            return directoryResult
        }
    }

    private fun controllerWith(source: FakeSource) =
        FileBrowseController(source, CoroutineScope(Dispatchers.Default))

    private fun awaitState(
        controller: FileBrowseController,
        predicate: (FileWorkspaceState) -> Boolean,
    ): FileWorkspaceState {
        val deadline = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < deadline) {
            val state = controller.state().value
            if (predicate(state)) return state
            Thread.sleep(5)
        }
        return controller.state().value
    }

    @Test
    fun openLoadsTheDirectoryAndCarriesTheTargetIdentity() {
        val source = FakeSource()
        source.directoryResult = BrowseResult.Ok(
            DirectoryPage(listOf(DirectoryEntry("docs", true, 0, null)), null, false),
        )
        val controller = controllerWith(source)
        controller.open(target, "docs")
        val state = awaitState(controller) { !it.loading && it.entries.isNotEmpty() }
        assertEquals(target, state.selected)
        assertEquals("docs", state.path)
        assertEquals(listOf("docs"), state.entries.map { it.path })
        assertEquals(listOf(BrowseRequest("peer-a", "share-1", "net-1", "docs", 100, null)), source.requests)
    }

    @Test
    fun invalidPathsNeverReachTheSource() {
        val source = FakeSource()
        val controller = controllerWith(source)
        controller.open(target, "/etc/passwd")
        val state = awaitState(controller) { it.errorKey != null }
        assertEquals("file.page.error.invalid_path", state.errorKey)
        assertTrue(source.requests.isEmpty())
    }

    @Test
    fun loadMoreResendsTheOpaqueCursorAndAppends() {
        val source = FakeSource()
        source.directoryResult = BrowseResult.Ok(
            DirectoryPage(listOf(DirectoryEntry("a", false, 1, null)), "c-2", true),
        )
        val controller = controllerWith(source)
        controller.open(target, "")
        awaitState(controller) { it.nextCursor == "c-2" }

        source.directoryResult = BrowseResult.Ok(
            DirectoryPage(listOf(DirectoryEntry("b", false, 1, null)), null, false),
        )
        controller.loadMore()
        val state = awaitState(controller) { !it.loading && it.entries.size == 2 }
        assertEquals(listOf("a", "b"), state.entries.map { it.path })
        assertEquals("c-2", source.requests.last().cursor)
    }

    @Test
    fun failuresBecomeSurfaceCopyAndKeepExistingEntries() {
        val source = FakeSource()
        source.directoryResult = BrowseResult.Ok(
            DirectoryPage(listOf(DirectoryEntry("a", false, 1, null)), "c-2", true),
        )
        val controller = controllerWith(source)
        controller.open(target, "")
        awaitState(controller) { it.entries.isNotEmpty() }

        source.directoryResult = BrowseResult.Failed(BridgeFailure.HttpStatus(429))
        controller.loadMore()
        val state = awaitState(controller) { it.errorKey != null }
        assertEquals("file.page.error.rate_limited_detail", state.errorKey)
        assertEquals(listOf("a"), state.entries.map { it.path })
    }

    @Test
    fun targetsFailureIsReportedWithoutInventingTargets() {
        val source = FakeSource()
        source.targetsResult = BrowseResult.Failed(BridgeFailure.Unreachable)
        val controller = controllerWith(source)
        controller.loadTargets()
        val state = awaitState(controller) { it.errorKey != null }
        assertEquals("file.page.error.unavailable_detail", state.errorKey)
        assertTrue(state.targets.isEmpty())
    }
}
