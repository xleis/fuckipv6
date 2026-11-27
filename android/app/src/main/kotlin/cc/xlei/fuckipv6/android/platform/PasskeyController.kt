package cc.xlei.fuckipv6.android.platform

import android.app.KeyguardManager
import android.content.Context
import android.net.Uri
import android.os.Build
import java.util.Locale

/**
 * Passkey / Credential Manager security & policy layer (W16-03).
 *
 * Pure policy decisions live in [PasskeyPolicy] so every edge case (no screen
 * lock, no biometrics, missing/migrated passkey, re-auth window expiry, origin
 * validation for Custom Tabs/WebView, secret redaction and cache wiping on
 * session revocation) is unit-tested without mocking Android framework classes.
 */
enum class PasskeyErrorKind(val messageKey: String) {
    NO_SCREEN_LOCK("android.passkey.error.no_screen_lock"),
    NO_BIOMETRICS("android.passkey.error.no_biometrics"),
    NO_CREDENTIAL("android.passkey.error.no_credential"),
    DEVICE_MIGRATION_REQUIRED("android.passkey.error.migration_required"),
    USER_CANCELLED("android.passkey.error.cancelled"),
    UNSUPPORTED_ORIGIN("android.passkey.error.invalid_origin"),
    PROVIDER_UNAVAILABLE("android.passkey.error.provider_unavailable"),
}

sealed interface PasskeyOutcome {
    data class Verified(
        val credentialId: String,
        val authenticatedAtUnixMs: Long,
        val responseJson: String,
    ) : PasskeyOutcome

    data class Failed(
        val kind: PasskeyErrorKind,
        val requiresLocalRecovery: Boolean = false,
    ) : PasskeyOutcome
}

data class DeviceAuthCapabilities(
    val sdkInt: Int,
    val hasDeviceScreenLock: Boolean,
    val hasCredentialManager: Boolean,
    val hasBiometricHardware: Boolean,
)

object PasskeyPolicy {
    /** Dangerous operations require re-authentication within the last 5 minutes (W3/W16-03). */
    const val RECENT_REAUTH_WINDOW_MS: Long = 5L * 60L * 1_000L

    private val SENSITIVE_PATTERNS = listOf(
        Regex("(?i)(init[_-]?code|bootstrap[_-]?code|bearer|token|challenge|clientDataJSON|attestationObject|signature)[=:\"\\s]+[^\\s,\"&}]+"),
        Regex("(?i)fv6[_-]init[_-][A-Za-z0-9_-]+"),
    )

    fun checkDeviceReadiness(capabilities: DeviceAuthCapabilities): PasskeyErrorKind? = when {
        !capabilities.hasDeviceScreenLock -> PasskeyErrorKind.NO_SCREEN_LOCK
        !capabilities.hasCredentialManager -> PasskeyErrorKind.PROVIDER_UNAVAILABLE
        else -> null
    }

    fun isRecentReauthValid(lastAuthUnixMs: Long?, nowUnixMs: Long): Boolean {
        if (lastAuthUnixMs == null || lastAuthUnixMs <= 0L) return false
        val elapsed = nowUnixMs - lastAuthUnixMs
        return elapsed in 0..RECENT_REAUTH_WINDOW_MS
    }

    /**
     * Validates a Custom Tab / WebView callback URL against the configured HTTPS
     * Relying Party origin. Rejects cleartext HTTP (except loopback diagnostics),
     * mismatched host/scheme/port, and any URL carrying an init code or token in
     * query/fragment parameters.
     */
    fun validateCallbackOrigin(callbackUrl: String, expectedOrigin: String): Boolean {
        val cb = parseOriginUri(callbackUrl) ?: return false
        val expected = parseOriginUri(expectedOrigin) ?: return false
        if (cb.scheme != "https" || expected.scheme != "https") return false
        if (!cb.host.equals(expected.host, ignoreCase = true)) return false
        if (cb.port != expected.port) return false
        if (containsForbiddenUrlSecret(callbackUrl)) return false
        return true
    }

    /** Refuses any URL or Intent payload that leaks initialization codes or bearer tokens. */
    fun containsForbiddenUrlSecret(raw: String): Boolean {
        val lower = raw.lowercase(Locale.ROOT)
        return lower.contains("init_code=") ||
            lower.contains("initcode=") ||
            lower.contains("bootstrap_code=") ||
            lower.contains("access_token=") ||
            lower.contains("bearer=") ||
            lower.contains("fv6_init_")
    }

    /** Redacts any init code, token or WebAuthn secret material before logging or diagnostics. */
    fun redactForLog(input: String): String {
        var sanitized = input
        for (pattern in SENSITIVE_PATTERNS) {
            sanitized = pattern.replace(sanitized, "[REDACTED]")
        }
        return sanitized
    }

    /** Classifies Android Credential Manager exception type/message into a closed error set. */
    fun classifyPlatformException(typeName: String, message: String?): PasskeyOutcome.Failed {
        val hay = "$typeName ${message.orEmpty()}".lowercase(Locale.ROOT)
        return when {
            hay.contains("cancellation") || hay.contains("canceled") || hay.contains("cancelled") ->
                PasskeyOutcome.Failed(PasskeyErrorKind.USER_CANCELLED)
            hay.contains("nocredential") || hay.contains("no_credential") || hay.contains("not found") ->
                PasskeyOutcome.Failed(
                    PasskeyErrorKind.NO_CREDENTIAL,
                    requiresLocalRecovery = true,
                )
            hay.contains("screen lock") || hay.contains("keyguard") || hay.contains("device_not_secured") ->
                PasskeyOutcome.Failed(PasskeyErrorKind.NO_SCREEN_LOCK)
            hay.contains("biometric") ->
                PasskeyOutcome.Failed(PasskeyErrorKind.NO_BIOMETRICS)
            hay.contains("origin") || hay.contains("rp id") || hay.contains("assetlinks") ->
                PasskeyOutcome.Failed(PasskeyErrorKind.UNSUPPORTED_ORIGIN)
            else ->
                PasskeyOutcome.Failed(PasskeyErrorKind.PROVIDER_UNAVAILABLE)
        }
    }

    private data class ParsedOrigin(val scheme: String, val host: String, val port: Int)

    private fun parseOriginUri(raw: String): ParsedOrigin? {
        val trimmed = raw.trim()
        val schemeIdx = trimmed.indexOf("://")
        if (schemeIdx <= 0) return null
        val scheme = trimmed.substring(0, schemeIdx).lowercase(Locale.ROOT)
        val afterScheme = trimmed.substring(schemeIdx + 3)
        val authority = afterScheme.substringBefore('/').substringBefore('?').substringBefore('#')
        if (authority.isEmpty() || authority.contains('@')) return null
        val host: String
        val port: Int
        val colonIdx = authority.lastIndexOf(':')
        if (colonIdx > 0 && !authority.endsWith(']')) {
            host = authority.substring(0, colonIdx)
            port = authority.substring(colonIdx + 1).toIntOrNull() ?: return null
        } else {
            host = authority
            port = if (scheme == "https") 443 else 80
        }
        if (host.isEmpty()) return null
        return ParsedOrigin(scheme, host, port)
    }
}

/**
 * In-memory session/re-auth cache cleared immediately on session revocation
 * (W16-03 #7). Never stores initialization codes on disk.
 */
class AndroidPasskeySessionStore(
    private val context: Context,
) {
    @Volatile
    private var lastReauthUnixMs: Long? = null

    @Volatile
    private var activeCredentialId: String? = null

    fun capabilities(): DeviceAuthCapabilities {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        val secured = runCatching { keyguard?.isDeviceSecure == true }.getOrDefault(false)
        val hasBiometric = runCatching {
            context.packageManager.hasSystemFeature("android.hardware.fingerprint") ||
                (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q &&
                    context.packageManager.hasSystemFeature("android.hardware.biometrics.face"))
        }.getOrDefault(false)
        val hasCredMan = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && isCredentialManagerClassAvailable()
        return DeviceAuthCapabilities(
            sdkInt = Build.VERSION.SDK_INT,
            hasDeviceScreenLock = secured,
            hasCredentialManager = hasCredMan,
            hasBiometricHardware = hasBiometric,
        )
    }

    fun recordVerified(outcome: PasskeyOutcome.Verified) {
        lastReauthUnixMs = outcome.authenticatedAtUnixMs
        activeCredentialId = outcome.credentialId
    }

    fun hasRecentReauth(nowUnixMs: Long = System.currentTimeMillis()): Boolean =
        PasskeyPolicy.isRecentReauthValid(lastReauthUnixMs, nowUnixMs)

    fun activeCredential(): String? = activeCredentialId

    /** Clears all local sensitive authentication cache when a session is revoked. */
    fun clearSensitiveSessionCache() {
        lastReauthUnixMs = null
        activeCredentialId = null
        context.getSharedPreferences(AUTH_CACHE_PREFS, Context.MODE_PRIVATE)
            .edit()
            .clear()
            .apply()
    }

    /**
     * Launches Android Credential Manager (`androidx.credentials.CredentialManager`)
     * via typed request/response when linked, degrading cleanly on devices without
     * a Passkey provider instead of crashing or falling back to a weak password.
     */
    suspend fun signInWithPasskey(
        activityContext: Context,
        requestJson: String,
    ): PasskeyOutcome {
        val readinessError = PasskeyPolicy.checkDeviceReadiness(capabilities())
        if (readinessError != null) {
            return PasskeyOutcome.Failed(
                readinessError,
                requiresLocalRecovery = readinessError == PasskeyErrorKind.NO_CREDENTIAL,
            )
        }
        return runCatching {
            val manager = androidx.credentials.CredentialManager.create(activityContext)
            val option = androidx.credentials.GetPublicKeyCredentialOption(requestJson = requestJson)
            val request = androidx.credentials.GetCredentialRequest(listOf(option))
            val result = manager.getCredential(activityContext, request)
            val cred = result.credential as? androidx.credentials.PublicKeyCredential
                ?: return PasskeyOutcome.Failed(PasskeyErrorKind.NO_CREDENTIAL, requiresLocalRecovery = true)
            val responseJson = cred.authenticationResponseJson
            val credId = runCatching {
                org.json.JSONObject(responseJson).optString("id", "")
            }.getOrDefault("")
            val verified = PasskeyOutcome.Verified(
                credentialId = credId.ifBlank { "passkey" },
                authenticatedAtUnixMs = System.currentTimeMillis(),
                responseJson = responseJson,
            )
            recordVerified(verified)
            verified
        }.getOrElse { error ->
            PasskeyPolicy.classifyPlatformException(error.javaClass.name, error.message)
        }
    }

    suspend fun registerPasskey(
        activityContext: Context,
        requestJson: String,
    ): PasskeyOutcome {
        val readinessError = PasskeyPolicy.checkDeviceReadiness(capabilities())
        if (readinessError != null) {
            return PasskeyOutcome.Failed(readinessError)
        }
        return runCatching {
            val manager = androidx.credentials.CredentialManager.create(activityContext)
            val request = androidx.credentials.CreatePublicKeyCredentialRequest(requestJson = requestJson)
            val response = manager.createCredential(activityContext, request)
            val created = response as? androidx.credentials.CreatePublicKeyCredentialResponse
                ?: return PasskeyOutcome.Failed(PasskeyErrorKind.PROVIDER_UNAVAILABLE)
            val responseJson = created.registrationResponseJson
            val credId = runCatching {
                org.json.JSONObject(responseJson).optString("id", "")
            }.getOrDefault("")
            val verified = PasskeyOutcome.Verified(
                credentialId = credId.ifBlank { "passkey" },
                authenticatedAtUnixMs = System.currentTimeMillis(),
                responseJson = responseJson,
            )
            recordVerified(verified)
            verified
        }.getOrElse { error ->
            PasskeyPolicy.classifyPlatformException(error.javaClass.name, error.message)
        }
    }

    private fun isCredentialManagerClassAvailable(): Boolean = runCatching {
        Class.forName("androidx.credentials.CredentialManager")
        true
    }.getOrDefault(false)

    companion object {
        private const val AUTH_CACHE_PREFS = "fuckipv6.auth.ephemeral"
    }
}
