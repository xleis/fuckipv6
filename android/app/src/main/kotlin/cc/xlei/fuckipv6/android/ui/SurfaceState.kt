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
}

data class SurfaceState(val status: SurfaceStatus) {
    val titleKey: String = "state.${status.wireName}.title"
    val detailKey: String = "state.${status.wireName}.detail"
}
