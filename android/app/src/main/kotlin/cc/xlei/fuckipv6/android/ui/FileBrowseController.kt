package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BrowseRequest
import cc.xlei.fuckipv6.android.bridge.BrowseResult
import cc.xlei.fuckipv6.android.bridge.BrowseSource
import cc.xlei.fuckipv6.android.bridge.FileTarget
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Drives [FileWorkspaceState] from a [BrowseSource].
 *
 * Requests run off the main thread, capture the generation they started with
 * and are dropped if the operator moved on. Nothing here writes: browsing is
 * read-only until the transfer surface is mounted.
 */
class FileBrowseController(
    private val source: BrowseSource,
    private val scope: CoroutineScope,
) {
    private val state = MutableStateFlow(FileWorkspaceState())

    fun state(): StateFlow<FileWorkspaceState> = state.asStateFlow()

    fun loadTargets() {
        scope.launch {
            val result = withContext(Dispatchers.IO) { source.targets() }
            state.value = when (result) {
                is BrowseResult.Ok -> state.value.targetsLoaded(result.value)
                is BrowseResult.Failed -> state.value.targetsFailed(result.failure)
            }
        }
    }

    fun open(target: FileTarget, path: String = "") {
        if (!SharedPathNav.isValid(path)) {
            state.value = state.value.copy(errorKey = "file.page.error.invalid_path")
            return
        }
        val next = state.value.loadingDirectory(target, path)
        state.value = next
        fetch(target, path, next.generation, append = false)
    }

    fun up() {
        val current = state.value
        val target = current.selected ?: return
        open(target, SharedPathNav.parent(current.path))
    }

    fun loadMore() {
        val current = state.value
        val target = current.selected ?: return
        val cursor = current.nextCursor ?: return
        val next = current.loadingMore()
        if (next === current) return
        state.value = next
        fetch(target, current.path, next.generation, append = true, cursor = cursor)
    }

    private fun fetch(
        target: FileTarget,
        path: String,
        generation: Long,
        append: Boolean,
        cursor: String? = null,
    ) {
        scope.launch {
            val request = BrowseRequest(
                peer = target.peer,
                shareId = target.shareId,
                networkId = target.networkId,
                path = path,
                cursor = cursor,
            )
            val result = withContext(Dispatchers.IO) { source.directory(request) }
            val current = state.value
            state.value = when (result) {
                is BrowseResult.Ok -> current.applying(result.value, generation)
                is BrowseResult.Failed -> current.failing(result.failure, generation)
            }
            if (append && generation != state.value.generation) {
                // A newer directory load superseded this page; nothing to do.
                Unit
            }
        }
    }
}
