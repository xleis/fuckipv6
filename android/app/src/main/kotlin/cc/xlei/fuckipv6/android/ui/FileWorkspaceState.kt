package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.DirectoryEntry
import cc.xlei.fuckipv6.android.bridge.DirectoryPage
import cc.xlei.fuckipv6.android.bridge.FileTarget

/**
 * Portable-path rules for the browser UI: a shared path is a relative sequence
 * of components with no `.`/`..`, no leading or trailing separator and no
 * backslash. The node re-validates everything; this only keeps the UI from
 * building a request it already knows is invalid.
 */
object SharedPathNav {
    fun join(parent: String, child: String): String = when {
        parent.isEmpty() -> child
        child.isEmpty() -> parent
        else -> "$parent/$child"
    }

    fun parent(path: String): String {
        val index = path.lastIndexOf('/')
        return if (index <= 0) "" else path.substring(0, index)
    }

    fun isRoot(path: String): Boolean = path.isEmpty()

    fun isValid(path: String): Boolean {
        if (path.isEmpty()) return true
        if (path.startsWith("/") || path.endsWith("/")) return false
        if (path.contains('\\')) return false
        return path.split('/').all { component ->
            component.isNotEmpty() &&
                component != "." &&
                component != ".." &&
                component.none { it.isISOControl() }
        }
    }
}

/** Browser state. Every field is authoritative UI detail, never a claim. */
data class FileWorkspaceState(
    val targets: List<FileTarget> = emptyList(),
    val selected: FileTarget? = null,
    val path: String = "",
    val entries: List<DirectoryEntry> = emptyList(),
    val nextCursor: String? = null,
    val truncated: Boolean = false,
    val loading: Boolean = false,
    /** True while loading the next page of the same directory. */
    val appending: Boolean = false,
    val errorKey: String? = null,
    val generation: Long = 0,
) {
    val canGoUp: Boolean
        get() = !SharedPathNav.isRoot(path)

    val canLoadMore: Boolean
        get() = truncated && nextCursor != null && !loading

    /** A new target/directory request: increments the stale-response guard. */
    fun loadingDirectory(target: FileTarget, path: String): FileWorkspaceState {
        require(SharedPathNav.isValid(path)) { "invalid shared path: $path" }
        return copy(
            selected = target,
            path = path,
            entries = emptyList(),
            nextCursor = null,
            truncated = false,
            loading = true,
            appending = false,
            errorKey = null,
            generation = generation + 1,
        )
    }

    fun loadingMore(): FileWorkspaceState =
        if (canLoadMore) copy(loading = true, appending = true, errorKey = null) else this

    /**
     * Applies a directory page only when it belongs to the current request.
     * A late response from a previous directory must never replace the page the
     * operator is looking at.
     */
    fun applying(page: DirectoryPage, generation: Long): FileWorkspaceState {
        if (generation != this.generation) return this
        return copy(
            entries = if (appending) entries + page.entries else page.entries,
            nextCursor = page.nextCursor,
            truncated = page.truncated,
            loading = false,
            appending = false,
            errorKey = null,
        )
    }

    fun failing(failure: BridgeFailure, generation: Long): FileWorkspaceState {
        if (generation != this.generation) return this
        return copy(
            loading = false,
            appending = false,
            errorKey = failure.browseErrorKey(),
        )
    }

    fun targetsLoaded(targets: List<FileTarget>): FileWorkspaceState =
        copy(targets = targets, loading = false, errorKey = null)

    fun targetsFailed(failure: BridgeFailure): FileWorkspaceState =
        copy(loading = false, errorKey = failure.browseErrorKey())
}

/** Reuses the shared File error copy instead of inventing new strings. */
fun BridgeFailure.browseErrorKey(): String = when (this) {
    is BridgeFailure.Unauthorized -> "file.page.error.permission_detail"
    BridgeFailure.Misconfigured -> "file.page.error.unsupported_detail"
    BridgeFailure.Malformed -> "file.page.error.generic"
    BridgeFailure.Unreachable -> "file.page.error.unavailable_detail"
    is BridgeFailure.HttpStatus ->
        if (status == 429) "file.page.error.rate_limited_detail" else "file.page.error.unavailable_detail"
}