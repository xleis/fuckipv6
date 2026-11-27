package cc.xlei.fuckipv6.android.bridge

import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import org.json.JSONException
import org.json.JSONObject

/**
 * Outcome of one command submission or operation poll.
 *
 * `InProgress` is not success: mutation commands register an operation before
 * they execute, and only a terminal `Succeeded` state with the authoritative
 * operation stage may be reported as done. `Rejected` carries the node's stable
 * problem code and message key so the UI can explain the refusal instead of
 * inventing one.
 */
sealed interface CommandOutcome {
    data class Succeeded(
        val operationId: String?,
        val stage: String,
        val outcomeStatus: String? = null,
        val data: JSONObject? = null,
    ) : CommandOutcome
    data class InProgress(val operationId: String?, val state: String, val stage: String) : CommandOutcome
    data class Rejected(
        val problemCode: String,
        val messageKey: String,
        val retryable: Boolean,
    ) : CommandOutcome
    data class Failed(val failure: BridgeFailure) : CommandOutcome

    val isTerminalSuccess: Boolean
        get() = this is Succeeded
}

/** Parses `POST /api/v1/commands` and `GET /api/v1/operations/{id}` bodies. */
object CommandResponseParser {
    fun parseCommandResponse(body: String): CommandOutcome = runCatching {
        val envelope = JSONObject(body)
        envelope.optJSONObject("operation")?.let { return@runCatching fromOperation(it) }
        envelope.optJSONObject("result")?.let { result ->
            val outcome = result.optJSONObject("outcome")
            return@runCatching CommandOutcome.Succeeded(
                operationId = null,
                stage = outcome?.optStringOrNull("status") ?: result.optStringOrNull("stage") ?: "result",
                outcomeStatus = outcome?.optStringOrNull("status"),
                data = outcome?.opt("data")?.let { value ->
                    when (value) {
                        is JSONObject -> value
                        is org.json.JSONArray -> JSONObject().put("items", value)
                        else -> null
                    }
                },
            )
        }
        // A 200 without an operation or result is not a success claim.
        CommandOutcome.Failed(BridgeFailure.Malformed)
    }.getOrElse { CommandOutcome.Failed(BridgeFailure.Malformed) }

    fun parseOperation(body: String): CommandOutcome = runCatching {
        fromOperation(JSONObject(body).optJSONObject("operation") ?: JSONObject(body))
    }.getOrElse { CommandOutcome.Failed(BridgeFailure.Malformed) }

    fun parseProblem(body: String): CommandOutcome.Rejected {
        val problem = runCatching { JSONObject(body).optJSONObject("problem") }.getOrNull()
            ?: runCatching { JSONObject(body) }.getOrNull()
        val code = problem?.optStringOrNull("code")
            ?: problem?.optStringOrNull("error_code")
            ?: "node.error.rejected"
        val messageKey = problem?.optStringOrNull("message_key")
            ?: problem?.optStringOrNull("message")
            ?: "state.error.detail"
        // Reuse an existing surface key when the node speaks in prose only.
        val safeKey = if (messageKey.startsWith("state.") || messageKey.startsWith("node.")) {
            messageKey
        } else {
            "state.error.detail"
        }
        return CommandOutcome.Rejected(
            problemCode = code,
            messageKey = safeKey,
            retryable = problem?.optBoolean("retryable", false) ?: false,
        )
    }

    private fun fromOperation(operation: JSONObject): CommandOutcome {
        val id = operation.optStringOrNull("operation_id")
        val stage = operation.optStringOrNull("stage") ?: "unknown"
        val state = operation.optStringOrNull("state") ?: "unknown"
        return when (state) {
            "succeeded" -> CommandOutcome.Succeeded(
                operationId = id,
                stage = stage,
                outcomeStatus = operation.optJSONObject("result")
                    ?.optJSONObject("outcome")
                    ?.optStringOrNull("status"),
                data = operation.optJSONObject("result")
                    ?.optJSONObject("outcome")
                    ?.opt("data")
                    ?.let { value ->
                        when (value) {
                            is JSONObject -> value
                            is org.json.JSONArray -> JSONObject().put("items", value)
                            else -> null
                        }
                    },
            )
            "failed", "cancelled", "timed_out" -> {
                val error = operation.optJSONObject("error")
                CommandOutcome.Rejected(
                    problemCode = error?.optStringOrNull("code") ?: "node.error.operation_$state",
                    messageKey = error?.optStringOrNull("message_key") ?: "state.error.detail",
                    retryable = error?.optBoolean("retryable", false) ?: false,
                )
            }
            else -> CommandOutcome.InProgress(id, state, stage)
        }
    }
}

private fun JSONObject.optStringOrNull(key: String): String? {
    if (!has(key) || isNull(key)) return null
    return optString(key, "").takeIf { it.isNotEmpty() }
}

/**
 * Blocking HTTP transport for node commands.
 *
 * No redirect following, no retry, no local "assume ok": every branch returns a
 * value the UI can render.
 */
interface CommandTransport {
    fun submit(body: String): CommandOutcome
    fun operation(operationId: String): CommandOutcome
}

class NodeCommandClient(
    private val config: NodeApiConfig,
    private val timeoutMillis: Int = 6_000,
) : CommandTransport {
    override fun submit(body: String): CommandOutcome =
        exchange(CommandJson.commandsUrl(config), body) { status, responseBody ->
            when {
                status == HttpURLConnection.HTTP_OK || status == HttpURLConnection.HTTP_ACCEPTED ->
                    CommandResponseParser.parseCommandResponse(responseBody)
                status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN ->
                    CommandOutcome.Failed(BridgeFailure.Unauthorized(status))
                status in 400..499 -> CommandResponseParser.parseProblem(responseBody)
                else -> CommandOutcome.Failed(BridgeFailure.HttpStatus(status))
            }
        }

    override fun operation(operationId: String): CommandOutcome =
        exchange(CommandJson.operationUrl(config, operationId), null) { status, responseBody ->
            when {
                status == HttpURLConnection.HTTP_OK -> CommandResponseParser.parseOperation(responseBody)
                status == HttpURLConnection.HTTP_UNAUTHORIZED || status == HttpURLConnection.HTTP_FORBIDDEN ->
                    CommandOutcome.Failed(BridgeFailure.Unauthorized(status))
                status == HttpURLConnection.HTTP_NOT_FOUND ->
                    CommandOutcome.Rejected("node.error.operation_not_found", "state.error.detail", false)
                else -> CommandOutcome.Failed(BridgeFailure.HttpStatus(status))
            }
        }

    private fun exchange(
        url: String,
        body: String?,
        map: (Int, String) -> CommandOutcome,
    ): CommandOutcome {
        config.validationError()?.let { return CommandOutcome.Failed(BridgeFailure.Misconfigured) }
        val connection = try {
            URL(url).openConnection() as HttpURLConnection
        } catch (_: IOException) {
            return CommandOutcome.Failed(BridgeFailure.Unreachable)
        } catch (_: IllegalArgumentException) {
            return CommandOutcome.Failed(BridgeFailure.Misconfigured)
        }
        return try {
            connection.requestMethod = if (body == null) "GET" else "POST"
            connection.connectTimeout = timeoutMillis
            connection.readTimeout = timeoutMillis
            connection.instanceFollowRedirects = false
            connection.setRequestProperty("Accept", "application/json")
            config.token?.takeIf { it.isNotBlank() }?.let {
                connection.setRequestProperty("Authorization", "Bearer $it")
            }
            if (body != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toByteArray(Charsets.UTF_8)) }
            }
            val status = connection.responseCode
            val stream = if (status in 200..299) connection.inputStream else connection.errorStream
            val responseBody = stream?.bufferedReader()?.use { it.readText() }.orEmpty()
            if (responseBody.isBlank() && status !in 200..299) {
                return CommandOutcome.Failed(BridgeFailure.HttpStatus(status))
            }
            map(status, responseBody)
        } catch (_: IOException) {
            CommandOutcome.Failed(BridgeFailure.Unreachable)
        } catch (_: JSONException) {
            CommandOutcome.Failed(BridgeFailure.Malformed)
        } finally {
            connection.disconnect()
        }
    }
}
