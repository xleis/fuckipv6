package cc.xlei.fuckipv6.android.ui

enum class SurfaceStatus(val wireName: String) {
    INITIAL_LOADING("initial_loading"),
    ERROR("error"),
    MAINTENANCE("maintenance"),
    PERMISSION_DENIED("permission_denied"),
    UNSUPPORTED("unsupported"),
    OFFLINE("offline"),
    CONTROL_UNAVAILABLE("control_unavailable"),
    NODE_FAILED("node_failed"),
    DEGRADED("degraded"),
    STALE("stale"),
    REFRESHING("refreshing"),
    EMPTY("empty"),
    READY("ready"),

    /** A page the desktop surfaces own and this device deliberately does not. */
    NOT_AVAILABLE_ON_DEVICE("not_available_on_device"),
}

data class SurfaceState(
    val status: SurfaceStatus,
    /**
     * Optional shared copy key for the detail line. Only used when a surface has
     * a more specific reason than its status; it must be an existing shared key,
     * never a freshly invented string.
     */
    val detailKeyOverride: String? = null,
) {
    val titleKey: String = "state.${status.wireName}.title"
    val detailKey: String = detailKeyOverride ?: "state.${status.wireName}.detail"
}

/**
 * Maps a bridge failure detail key to the status that headline should carry.
 *
 * A failed fetch used to render under "control service unavailable", which
 * describes a server-side control plane, not what the reader is looking at.
 * The detail key already names the real cause, so the title follows it.
 */
fun failureStatus(detailKey: String?): SurfaceStatus = when (detailKey) {
    "state.permission_denied.detail" -> SurfaceStatus.PERMISSION_DENIED
    "state.offline.detail" -> SurfaceStatus.OFFLINE
    "state.error.detail" -> SurfaceStatus.ERROR
    "state.unsupported.detail" -> SurfaceStatus.UNSUPPORTED
    else -> SurfaceStatus.CONTROL_UNAVAILABLE
}
