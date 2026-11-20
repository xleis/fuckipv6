package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.DownloadFetcher
import cc.xlei.fuckipv6.android.bridge.FetchOutcome
import cc.xlei.fuckipv6.android.bridge.HttpDownloadFetcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.OutputStream

/**
 * A destination the operator chose (SAF document, cache file, …).
 *
 * `open` returns the stream to write into, `discard` removes whatever was
 * written, and `close` finalises a complete file. The pipeline calls `discard`
 * on every failure so a broken download can never be presented as saved.
 */
interface DownloadSink {
    fun open(): OutputStream
    fun close()
    fun discard()
}

sealed interface DownloadState {
    data object Idle : DownloadState
    data class Saving(val token: String) : DownloadState
    data class Saved(val bytes: Long) : DownloadState
    data class Failed(val errorKey: String) : DownloadState
}

/**
 * Saves one token-bound download into a [DownloadSink].
 *
 * Bytes are only ever reported as saved after the fetcher finished a complete
 * stream and the sink closed successfully. Any failure discards the sink and
 * surfaces shared File error copy.
 */
class DownloadSaver(
    private val fetcher: DownloadFetcher,
    private val scope: CoroutineScope,
    private val maxBytes: Long = HttpDownloadFetcher.MAX_BYTES,
) {
    private val state = MutableStateFlow<DownloadState>(DownloadState.Idle)

    fun state(): StateFlow<DownloadState> = state.asStateFlow()

    fun save(token: String, sink: DownloadSink) {
        if (token.isBlank()) {
            state.value = DownloadState.Failed("file.page.error.generic")
            return
        }
        state.value = DownloadState.Saving(token)
        scope.launch {
            val outcome = withContext(Dispatchers.IO) {
                var opened = false
                try {
                    val stream = sink.open()
                    opened = true
                    val result = fetcher.fetch(token, stream, maxBytes)
                    when (result) {
                        is FetchOutcome.Ok -> {
                            stream.flush()
                            try {
                                sink.close()
                            } catch (_: Exception) {
                                sink.discard()
                                return@withContext DownloadState.Failed("file.page.error.generic")
                            }
                            DownloadState.Saved(result.bytesWritten)
                        }
                        is FetchOutcome.Failed -> {
                            sink.discard()
                            DownloadState.Failed(result.failure.downloadErrorKey())
                        }
                    }
                } catch (_: Exception) {
                    if (opened) sink.discard()
                    DownloadState.Failed("file.page.error.generic")
                }
            }
            state.value = outcome
        }
    }

    fun reset() {
        state.value = DownloadState.Idle
    }
}

/** Download failures reuse the shared File error copy. */
fun BridgeFailure.downloadErrorKey(): String = when (this) {
    is BridgeFailure.Unauthorized -> "file.page.error.permission_detail"
    BridgeFailure.Misconfigured -> "file.page.error.unsupported_detail"
    BridgeFailure.Malformed -> "file.page.error.generic"
    BridgeFailure.Unreachable -> "file.page.error.unavailable_detail"
    is BridgeFailure.HttpStatus -> when (status) {
        413 -> "file.page.error.download_too_large_detail"
        429 -> "file.page.error.rate_limited_detail"
        else -> "file.page.error.unavailable_detail"
    }
}