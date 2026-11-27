package cc.xlei.fuckipv6.android.platform

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SafShareManagerTest {
    @Test
    fun virtualAndUnknownLengthDocumentsCannotStreamWithFixedLength() {
        val normal = SafDocumentInspection("doc.pdf", 4096L, isVirtualDocument = false, isSeekable = true, mimeType = "application/pdf")
        assertTrue(normal.canStreamWithFixedLength)

        val virtual = normal.copy(isVirtualDocument = true)
        assertFalse(virtual.canStreamWithFixedLength)

        val unknownSize = normal.copy(sizeBytes = null)
        assertFalse(unknownSize.canStreamWithFixedLength)

        val oversized = normal.copy(sizeBytes = SafQuotaPolicy.MAX_SINGLE_FILE_BYTES + 1L)
        assertFalse(oversized.canStreamWithFixedLength)
    }

    @Test
    fun quotaPolicyEnforcesConcurrencyAndByteBounds() {
        assertNull(SafQuotaPolicy.isTransferAllowed(activeTransfers = 0, requestedBytes = 1024L, currentCacheBytes = 0L))
        assertNotNull(SafQuotaPolicy.isTransferAllowed(activeTransfers = SafQuotaPolicy.MAX_CONCURRENT_TRANSFERS, requestedBytes = 1024L, currentCacheBytes = 0L))
        assertNotNull(SafQuotaPolicy.isTransferAllowed(activeTransfers = 0, requestedBytes = -1L, currentCacheBytes = 0L))
        assertNotNull(SafQuotaPolicy.isTransferAllowed(activeTransfers = 0, requestedBytes = SafQuotaPolicy.MAX_SINGLE_FILE_BYTES + 1L, currentCacheBytes = 0L))
    }

    @Test
    fun permissionReconciliationDetectsRevokedTreeGrants() {
        val s1 = PersistedSafShare("content://com.android.externalstorage/tree/primary%3ADocs", "Docs", true, true, 100L, revoked = false)
        val s2 = PersistedSafShare("content://com.android.externalstorage/tree/primary%3APics", "Pics", true, false, 200L, revoked = false)
        val (updated, newlyRevoked) = SafQuotaPolicy.reconcilePermissions(
            stored = listOf(s1, s2),
            activePersistedUris = setOf(s1.uriString),
        )
        assertFalse(updated.first { it.uriString == s1.uriString }.revoked)
        assertTrue(updated.first { it.uriString == s2.uriString }.revoked)
        assertEquals(listOf(s2.uriString), newlyRevoked.map { it.uriString })
    }
}
