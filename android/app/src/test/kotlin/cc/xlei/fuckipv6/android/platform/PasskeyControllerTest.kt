package cc.xlei.fuckipv6.android.platform

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PasskeyControllerTest {
    @Test
    fun readinessRejectsMissingScreenLockOrProvider() {
        assertEquals(
            PasskeyErrorKind.NO_SCREEN_LOCK,
            PasskeyPolicy.checkDeviceReadiness(
                DeviceAuthCapabilities(36, hasDeviceScreenLock = false, hasCredentialManager = true, hasBiometricHardware = true),
            ),
        )
        assertEquals(
            PasskeyErrorKind.PROVIDER_UNAVAILABLE,
            PasskeyPolicy.checkDeviceReadiness(
                DeviceAuthCapabilities(36, hasDeviceScreenLock = true, hasCredentialManager = false, hasBiometricHardware = true),
            ),
        )
        assertNull(
            PasskeyPolicy.checkDeviceReadiness(
                DeviceAuthCapabilities(36, hasDeviceScreenLock = true, hasCredentialManager = true, hasBiometricHardware = false),
            ),
        )
    }

    @Test
    fun recentReauthWindowEnforcesFiveMinuteBound() {
        val t0 = 1_700_000_000_000L
        assertFalse(PasskeyPolicy.isRecentReauthValid(null, t0))
        assertTrue(PasskeyPolicy.isRecentReauthValid(t0, t0 + 60_000L))
        assertTrue(PasskeyPolicy.isRecentReauthValid(t0, t0 + PasskeyPolicy.RECENT_REAUTH_WINDOW_MS))
        assertFalse(PasskeyPolicy.isRecentReauthValid(t0, t0 + PasskeyPolicy.RECENT_REAUTH_WINDOW_MS + 1L))
        assertFalse(PasskeyPolicy.isRecentReauthValid(t0 + 1000L, t0))
    }

    @Test
    fun callbackOriginRejectsCleartextForeignHostAndSecretLeak() {
        assertTrue(
            PasskeyPolicy.validateCallbackOrigin(
                "https://node.example.com/auth/callback?state=ok",
                "https://node.example.com",
            ),
        )
        assertFalse(
            PasskeyPolicy.validateCallbackOrigin(
                "http://node.example.com/auth/callback",
                "https://node.example.com",
            ),
        )
        assertFalse(
            PasskeyPolicy.validateCallbackOrigin(
                "https://evil.example.com/auth/callback",
                "https://node.example.com",
            ),
        )
        assertFalse(
            PasskeyPolicy.validateCallbackOrigin(
                "https://node.example.com/auth/callback?init_code=fv6_init_secret123",
                "https://node.example.com",
            ),
        )
    }

    @Test
    fun redactionStripsInitCodeAndTokens() {
        val raw = "bootstrap init_code=fv6_init_abc123 bearer=tok_999 clientDataJSON=\"eyJ0\""
        val redacted = PasskeyPolicy.redactForLog(raw)
        assertFalse(redacted.contains("fv6_init_abc123"))
        assertFalse(redacted.contains("tok_999"))
        assertFalse(redacted.contains("eyJ0"))
        assertTrue(redacted.contains("[REDACTED]"))
    }

    @Test
    fun exceptionClassificationMapsMissingCredentialToLocalRecovery() {
        val noCred = PasskeyPolicy.classifyPlatformException("NoCredentialException", "no matching passkey")
        assertEquals(PasskeyErrorKind.NO_CREDENTIAL, noCred.kind)
        assertTrue(noCred.requiresLocalRecovery)

        val cancelled = PasskeyPolicy.classifyPlatformException("GetCredentialCancellationException", "user cancelled")
        assertEquals(PasskeyErrorKind.USER_CANCELLED, cancelled.kind)
        assertFalse(cancelled.requiresLocalRecovery)
    }
}
