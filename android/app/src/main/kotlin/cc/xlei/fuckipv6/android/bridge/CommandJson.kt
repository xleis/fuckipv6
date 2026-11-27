package cc.xlei.fuckipv6.android.bridge

import java.security.MessageDigest
import java.util.Base64
import org.json.JSONObject

/**
 * Wire encoding for node commands.
 *
 * The node protocol wraps every command as
 * `{schema_version, request {request_id, command {command, payload}}, idempotency_key?}`
 * and tags the command with `command`/`payload` (not `kind`). Getting that
 * shape wrong is a silent 400, so the encoding lives here with tests instead of
 * being string-built at the call site.
 */
object CommandJson {
    const val SCHEMA_VERSION = 1
    private val CANONICAL_IDEMPOTENCY_KEY = Regex("^idem_[A-Za-z0-9_-]{43}$")

    /**
     * Normalizes an arbitrary caller key into the authoritative Rust contract
     * format `^idem_[A-Za-z0-9_-]{43}$`. Already-valid keys pass through
     * untouched; any other non-blank label is deterministically hashed via
     * SHA-256 (32 bytes -> 43 base64url chars) so retries keep exact
     * idempotency semantics.
     */
    fun normalizeIdempotencyKey(raw: String): String {
        val trimmed = raw.trim()
        if (CANONICAL_IDEMPOTENCY_KEY.matches(trimmed)) return trimmed
        val digest = MessageDigest.getInstance("SHA-256").digest(trimmed.toByteArray(Charsets.UTF_8))
        val encoded = Base64.getUrlEncoder().withoutPadding().encodeToString(digest)
        return "idem_$encoded"
    }

    fun setNetworkRole(
        networkName: String,
        role: String,
        enabled: Boolean,
    ): JSONObject = JSONObject()
        .put("command", "set_network_role")
        .put(
            "payload",
            JSONObject()
                .put("network_name", networkName)
                .put("role", role)
                .put("enabled", enabled),
        )

    fun envelope(
        command: JSONObject,
        requestId: Long,
        idempotencyKey: String? = null,
    ): String {
        require(requestId > 0) { "request_id must be greater than 0" }
        val request = JSONObject()
            .put("request_id", requestId)
            .put("command", command)
        val envelope = JSONObject()
            .put("schema_version", SCHEMA_VERSION)
            .put("request", request)
        idempotencyKey?.takeIf { it.isNotBlank() }?.let {
            envelope.put("idempotency_key", normalizeIdempotencyKey(it))
        }
        return envelope.toString()
    }

    fun operationUrl(config: NodeApiConfig, operationId: String): String =
        config.baseUrl.trimEnd('/') + OPERATIONS_PATH + operationId

    const val COMMANDS_PATH = "/api/v1/commands"
    const val OPERATIONS_PATH = "/api/v1/operations/"

    fun commandsUrl(config: NodeApiConfig): String =
        config.baseUrl.trimEnd('/') + COMMANDS_PATH
}
