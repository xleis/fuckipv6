package cc.xlei.fuckipv6.android.platform

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import java.io.File

/**
 * SAF shared-directory permission, virtual/cloud document, quota and revocation
 * manager (W16-04).
 *
 * Android never assumes a SAF `content://` URI maps to a POSIX path. All share
 * tree access is persisted via `takePersistableUriPermission`, re-validated
 * against `contentResolver.persistedUriPermissions`, and aborted immediately
 * when permission is revoked.
 */
data class PersistedSafShare(
    val uriString: String,
    val displayLabel: String,
    val readGranted: Boolean,
    val writeGranted: Boolean,
    val grantedAtUnixMs: Long,
    val revoked: Boolean = false,
)

data class SafDocumentInspection(
    val displayName: String?,
    val sizeBytes: Long?,
    val isVirtualDocument: Boolean,
    val isSeekable: Boolean,
    val mimeType: String?,
) {
    /** Rejects virtual/non-seekable streams whose byte length cannot be verified. */
    val canStreamWithFixedLength: Boolean
        get() = !isVirtualDocument && sizeBytes != null && sizeBytes >= 0L && sizeBytes <= SafQuotaPolicy.MAX_SINGLE_FILE_BYTES
}

object SafQuotaPolicy {
    const val MAX_SINGLE_FILE_BYTES: Long = 512L * 1024L * 1024L
    const val MAX_CONCURRENT_TRANSFERS: Int = 4
    const val MAX_TEMP_CACHE_BYTES: Long = 128L * 1024L * 1024L
    const val TEMP_CACHE_TTL_MS: Long = 30L * 60L * 1_000L

    fun isTransferAllowed(
        activeTransfers: Int,
        requestedBytes: Long,
        currentCacheBytes: Long,
    ): String? = when {
        activeTransfers >= MAX_CONCURRENT_TRANSFERS -> "file.page.error.rate_limited_detail"
        requestedBytes < 0L -> "file.page.error.unsupported_detail"
        requestedBytes > MAX_SINGLE_FILE_BYTES -> "file.page.error.upload_too_large_detail"
        currentCacheBytes + requestedBytes.coerceAtMost(MAX_TEMP_CACHE_BYTES) > MAX_TEMP_CACHE_BYTES * 2 ->
            "file.page.error.unavailable_detail"
        else -> null
    }

    /** Computes which stored SAF shares lost OS permission since they were granted. */
    fun reconcilePermissions(
        stored: List<PersistedSafShare>,
        activePersistedUris: Set<String>,
    ): Pair<List<PersistedSafShare>, List<PersistedSafShare>> {
        val updated = stored.map { share ->
            val stillGranted = share.uriString in activePersistedUris
            share.copy(revoked = !stillGranted)
        }
        val newlyRevoked = updated.filter { it.revoked && stored.firstOrNull { old -> old.uriString == it.uriString }?.revoked == false }
        return updated to newlyRevoked
    }
}

class SafShareManager(
    private val context: Context,
    private val onRevoked: (List<PersistedSafShare>) -> Unit = {},
) {
    private val prefs = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
    private val tempCacheDir = File(context.cacheDir, "saf-staging")

    fun persistTreeGrant(treeUri: Uri, label: String? = null): PersistedSafShare {
        val flags = Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
        val writeOk = runCatching {
            context.contentResolver.takePersistableUriPermission(treeUri, flags)
            true
        }.getOrElse {
            runCatching {
                context.contentResolver.takePersistableUriPermission(
                    treeUri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION,
                )
            }
            false
        }
        val share = PersistedSafShare(
            uriString = treeUri.toString(),
            displayLabel = label?.takeIf { it.isNotBlank() } ?: treeUri.lastPathSegment ?: "SAF",
            readGranted = true,
            writeGranted = writeOk,
            grantedAtUnixMs = System.currentTimeMillis(),
            revoked = false,
        )
        val list = loadShares().filterNot { it.uriString == share.uriString } + share
        saveShares(list)
        return share
    }

    fun releaseTreeGrant(uriString: String) {
        val uri = runCatching { Uri.parse(uriString) }.getOrNull()
        if (uri != null) {
            runCatching {
                context.contentResolver.releasePersistableUriPermission(
                    uri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            }
        }
        saveShares(loadShares().filterNot { it.uriString == uriString })
        purgeTempCache()
    }

    /**
     * Re-checks persisted URI grants against Android's authoritative
     * `ContentResolver.persistedUriPermissions`. If any active share lost
     * permission, invokes [onRevoked] (stopping in-flight uploads/downloads and
     * resume state) and purges staged temp files.
     */
    fun verifyGrants(): List<PersistedSafShare> {
        val activeUris = runCatching {
            context.contentResolver.persistedUriPermissions
                .filter { it.isReadPermission }
                .map { it.uri.toString() }
                .toSet()
        }.getOrDefault(emptySet())
        val (updated, newlyRevoked) = SafQuotaPolicy.reconcilePermissions(loadShares(), activeUris)
        saveShares(updated)
        if (newlyRevoked.isNotEmpty()) {
            purgeTempCache()
            onRevoked(newlyRevoked)
        }
        return updated
    }

    fun inspectDocument(uri: Uri): SafDocumentInspection {
        val resolver = context.contentResolver
        val mime = runCatching { resolver.getType(uri) }.getOrNull()
        val name = resolver.displayName(uri)
        val size = resolver.uploadLength(uri)
        val virtual = isVirtualDocument(resolver, uri)
        val seekable = runCatching {
            resolver.openFileDescriptor(uri, "r")?.use { pfd -> pfd.statSize >= 0 } ?: false
        }.getOrDefault(false)
        return SafDocumentInspection(
            displayName = name,
            sizeBytes = size,
            isVirtualDocument = virtual,
            isSeekable = seekable,
            mimeType = mime,
        )
    }

    fun purgeTempCache(maxAgeMs: Long = 0L): Long {
        if (!tempCacheDir.exists()) return 0L
        val now = System.currentTimeMillis()
        var removedBytes = 0L
        tempCacheDir.listFiles()?.forEach { file ->
            if (maxAgeMs == 0L || now - file.lastModified() >= maxAgeMs) {
                removedBytes += file.length()
                file.delete()
            }
        }
        return removedBytes
    }

    fun currentTempCacheBytes(): Long {
        if (!tempCacheDir.exists()) return 0L
        return tempCacheDir.listFiles()?.sumOf { it.length() } ?: 0L
    }

    private fun isVirtualDocument(resolver: ContentResolver, uri: Uri): Boolean {
        if (!DocumentsContract.isDocumentUri(context, uri)) return false
        return runCatching {
            resolver.query(
                uri,
                arrayOf(DocumentsContract.Document.COLUMN_FLAGS),
                null,
                null,
                null,
            )?.use { cursor ->
                if (!cursor.moveToFirst()) return@use false
                val flags = cursor.getInt(0)
                (flags and DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT) != 0
            } ?: false
        }.getOrDefault(false)
    }

    fun loadShares(): List<PersistedSafShare> {
        val raw = prefs.getString(KEY_SHARES, null) ?: return emptyList()
        return runCatching {
            val arr = org.json.JSONArray(raw)
            (0 until arr.length()).mapNotNull { idx ->
                val obj = arr.optJSONObject(idx) ?: return@mapNotNull null
                val uri = obj.optString("uri", "").takeIf { it.isNotBlank() } ?: return@mapNotNull null
                PersistedSafShare(
                    uriString = uri,
                    displayLabel = obj.optString("label", "SAF"),
                    readGranted = obj.optBoolean("read", true),
                    writeGranted = obj.optBoolean("write", false),
                    grantedAtUnixMs = obj.optLong("granted_at", 0L),
                    revoked = obj.optBoolean("revoked", false),
                )
            }
        }.getOrDefault(emptyList())
    }

    private fun saveShares(shares: List<PersistedSafShare>) {
        val arr = org.json.JSONArray()
        shares.forEach { share ->
            arr.put(
                org.json.JSONObject()
                    .put("uri", share.uriString)
                    .put("label", share.displayLabel)
                    .put("read", share.readGranted)
                    .put("write", share.writeGranted)
                    .put("granted_at", share.grantedAtUnixMs)
                    .put("revoked", share.revoked),
            )
        }
        prefs.edit().putString(KEY_SHARES, arr.toString()).apply()
    }

    companion object {
        private const val PREFERENCES = "fuckipv6.saf"
        private const val KEY_SHARES = "persisted_tree_shares"
    }
}
