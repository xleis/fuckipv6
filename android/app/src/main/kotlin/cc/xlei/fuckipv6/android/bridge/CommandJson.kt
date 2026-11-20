package cc.xlei.fuckipv6.android.bridge

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
        idempotencyKey?.takeIf { it.isNotBlank() }?.let { envelope.put("idempotency_key", it) }
        return envelope.toString()
    }

    fun operationUrl(config: NodeApiConfig, operationId: String): String =
        config.baseUrl.trimEnd('/') + OPERATIONS_PATH + operationId

    const val COMMANDS_PATH = "/api/v1/commands"
    const val OPERATIONS_PATH = "/api/v1/operations/"

    fun commandsUrl(config: NodeApiConfig): String =
        config.baseUrl.trimEnd('/') + COMMANDS_PATH
}
