package cc.xlei.fuckipv6.android.ui

/**
 * Names the File surfaces derive locally.
 *
 * Everything is validated with the same portable-path rules the browser uses, so
 * a document provider's display name can never smuggle a path separator, a `..`
 * or a device name into an upload target.
 */
object FileTransferNaming {
    private const val FALLBACK_DOWNLOAD_NAME = "download.bin"

    /** Suggested save name for a shared path; never includes a directory part. */
    fun suggestFileName(sharedPath: String): String {
        val name = sharedPath.substringAfterLast('/').trim()
        return if (name.isEmpty() || !isSafeComponent(name)) FALLBACK_DOWNLOAD_NAME else name
    }

    /**
     * Upload target inside the current directory, or null when the provider's
     * name cannot be represented as a single safe path component.
     */
    fun uploadTargetPath(directory: String, displayName: String?): String? {
        val name = displayName?.trim()?.takeIf { it.isNotEmpty() } ?: return null
        if (!isSafeComponent(name)) return null
        val joined = SharedPathNav.join(directory, name)
        return joined.takeIf(SharedPathNav::isValid)
    }

    /**
     * DOS device names are rejected even on Android: the node may hand the path
     * to a Windows peer, where `CON`/`NUL`/`COM1` are not ordinary file names.
     */
    private val DEVICE_NAMES: Set<String> = buildSet {
        addAll(listOf("CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"))
        (1..9).forEach { suffix ->
            add("COM$suffix")
            add("LPT$suffix")
        }
        listOf("¹", "²", "³").forEach { superscript ->
            add("COM$superscript")
            add("LPT$superscript")
        }
    }

    private fun isSafeComponent(name: String): Boolean =
        name.substringBefore('.').uppercase() !in DEVICE_NAMES &&
            name != "." &&
            name != ".." &&
            '/' !in name &&
            '\\' !in name &&
            ':' !in name &&
            name.none { character -> character.isISOControl() }
}
