package cc.xlei.fuckipv6.android.platform

import android.content.ContentResolver
import android.net.Uri
import android.provider.OpenableColumns
import cc.xlei.fuckipv6.android.ui.DownloadSink
import java.io.InputStream
import java.io.OutputStream

/**
 * SAF-backed sinks and sources.
 *
 * The Android app never writes to a raw host path: the operator picks a document
 * through the system picker and this adapter writes into exactly that document.
 * A failed download deletes the document instead of leaving a truncated file
 * the user might mistake for a complete one.
 */
class SafDownloadSink(
    private val resolver: ContentResolver,
    private val uri: Uri,
) : DownloadSink {
    private var stream: OutputStream? = null

    override fun open(): OutputStream {
        val opened = resolver.openOutputStream(uri, "wt")
            ?: error("cannot open the selected document for writing")
        stream = opened
        return opened
    }

    override fun close() {
        stream?.flush()
        stream?.close()
        stream = null
    }

    override fun discard() {
        runCatching { stream?.close() }
        stream = null
        // Best effort: a document provider may refuse deletion, in which case the
        // partial file stays but is never reported as saved.
        runCatching { resolver.delete(uri, null, null) }
    }
}

/**
 * Opens a picked document for upload as a plain [InputStream]. The caller owns
 * the stream and must close it; the transfer client copies it once with a fixed
 * content length.
 */
fun ContentResolver.openUploadSource(uri: Uri): InputStream =
    openInputStream(uri) ?: error("cannot open the selected document for reading")

/**
 * Display name the provider reports for the picked document, or null when it
 * exposes none. Callers must not fall back to the raw URI path: that is a
 * provider-internal identifier, not a file name.
 */
fun ContentResolver.displayName(uri: Uri): String? =
    runCatching {
        query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (!cursor.moveToFirst()) {
                null
            } else {
                val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (index < 0) null else cursor.getString(index)
            }
        }
    }.getOrNull()?.takeIf { it.isNotBlank() }

/**
 * Best-effort byte count for the picked document, so uploads can send a fixed
 * content length instead of chunked encoding. Returns null when the provider
 * does not report a size; the caller must not guess one.
 */
fun ContentResolver.uploadLength(uri: Uri): Long? =
    runCatching {
        openAssetFileDescriptor(uri, "r")?.use { descriptor -> descriptor.length }
    }.getOrNull()?.takeIf { it >= 0 }