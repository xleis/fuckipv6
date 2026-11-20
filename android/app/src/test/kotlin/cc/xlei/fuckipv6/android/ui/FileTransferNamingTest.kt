package cc.xlei.fuckipv6.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FileTransferNamingTest {
    @Test
    fun downloadNameKeepsOnlyTheLastComponent() {
        assertEquals("a.md", FileTransferNaming.suggestFileName("docs/a.md"))
        assertEquals("报表.xlsx", FileTransferNaming.suggestFileName("报表.xlsx"))
        assertEquals("download.bin", FileTransferNaming.suggestFileName(""))
        assertEquals("download.bin", FileTransferNaming.suggestFileName("docs/"))
    }

    @Test
    fun unsafeProviderNamesAreRejectedInsteadOfSanitised() {
        assertEquals("docs/notes.txt", FileTransferNaming.uploadTargetPath("docs", "notes.txt"))
        assertEquals("notes.txt", FileTransferNaming.uploadTargetPath("", "notes.txt"))
        assertEquals("docs/sub/notes.txt", FileTransferNaming.uploadTargetPath("docs/sub", " notes.txt "))
        assertNull(FileTransferNaming.uploadTargetPath("docs", "../escape.txt"))
        assertNull(FileTransferNaming.uploadTargetPath("docs", "a/b.txt"))
        assertNull(FileTransferNaming.uploadTargetPath("docs", "a\\b.txt"))
        assertNull(FileTransferNaming.uploadTargetPath("docs", "CON"))
        assertNull(FileTransferNaming.uploadTargetPath("docs", "a:b.txt"))
        assertNull(FileTransferNaming.uploadTargetPath("docs", " "))
        assertNull(FileTransferNaming.uploadTargetPath("docs", null))
    }
}
