package cc.xlei.fuckipv6.android.bridge

import org.json.JSONException
import org.json.JSONObject

/**
 * One frame of `GET /api/v1/events`.
 *
 * The node streams JSON payloads as SSE `data:` lines with one of three types:
 * `snapshot` (authoritative snapshot plus resume cursor), `event` (a change
 * notification) and `resync_required` (the requested cursor is too old). The
 * Android bridge only needs to know which of those arrived and, for `snapshot`,
 * the snapshot itself.
 */
data class NodeEventFrame(
    val type: String,
    val cursor: String? = null,
    val latestCursor: String? = null,
    val sequence: Long? = null,
    val snapshotRevision: Long? = null,
    val payload: JSONObject? = null,
) {
    /**
     * Resume token in the node's `sequence:snapshot_revision` form. Without a
     * revision the node accepts a bare sequence, so a frame that only carries a
     * sequence still produces a usable token.
     */
    fun resumeToken(): String? {
        val seq = sequence ?: return cursor
        val revision = snapshotRevision
        return if (revision == null) seq.toString() else "$seq:$revision"
    }

    /**
     * What the bridge should do next. Events and resync both need a fresh
     * authoritative snapshot; only a `snapshot` frame can be applied directly.
     */
    fun action(): LiveAction = when (type) {
        "snapshot" -> payload?.let { LiveAction.ApplySnapshot(it) } ?: LiveAction.RefreshSnapshot
        "event" -> LiveAction.RefreshSnapshot
        "resync_required" -> LiveAction.Resync
        else -> LiveAction.Ignore
    }
}

sealed interface LiveAction {
    /** The frame carried an authoritative snapshot. */
    data class ApplySnapshot(val snapshot: JSONObject) : LiveAction

    /** Something changed; pull the authoritative snapshot. */
    data object RefreshSnapshot : LiveAction

    /** The cursor is too old; pull the snapshot and keep the new cursor. */
    data object Resync : LiveAction

    data object Ignore : LiveAction
}

object EventFrameParser {
    /** Cursor text as the node reports it, when it is a plain string. */
    private fun JSONObject?.cursorText(): String? =
        this?.stringOrNull("cursor") ?: this?.longOrNull("sequence")?.toString()

    private const val DATA_PREFIX = "data:"

    /**
     * Parses one SSE `data:` line. Blank lines, comments, `id:`/`event:`/`retry:`
     * fields and unparseable payloads are ignored (the caller keeps reading);
     * a frame is only produced when its `type` is one this client understands.
     */
    fun parse(line: String): NodeEventFrame? {
        val trimmed = line.trim()
        if (trimmed.isEmpty() || trimmed.startsWith(":")) return null
        if (!trimmed.startsWith(DATA_PREFIX)) return null
        val payload = trimmed.removePrefix(DATA_PREFIX).trim()
        if (payload.isEmpty()) return null
        val json = try {
            JSONObject(payload)
        } catch (_: JSONException) {
            return null
        }
        val type = json.stringOrNull("type") ?: return null
        if (type !in SUPPORTED_TYPES) return null
        val cursorObject = json.optJSONObject("cursor")
        val latestObject = json.optJSONObject("latest_cursor")
        return NodeEventFrame(
            type = type,
            cursor = cursorObject.cursorText(),
            latestCursor = latestObject.cursorText(),
            sequence = cursorObject?.longOrNull("sequence"),
            snapshotRevision = cursorObject?.longOrNull("snapshot_revision"),
            payload = json.optJSONObject("snapshot"),
        )
    }

    /** The stream frame carries the snapshot object itself, not an envelope. */
    fun parseSnapshot(payload: JSONObject): Result<ParsedSnapshot> =
        SnapshotJsonParser.parseSnapshot(
            JSONObject().put("snapshot", payload).toString(),
        )

    private val SUPPORTED_TYPES = setOf("snapshot", "event", "resync_required")
}