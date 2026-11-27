package cc.xlei.fuckipv6.android.bridge

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CommandBridgeTest {
    private val config = NodeApiConfig("http://127.0.0.1:6668")

    @Test
    fun commandEnvelopeUsesTheDocumentedTagAndPayload() {
        val command = CommandJson.setNetworkRole(
            networkName = "家庭网络",
            role = "relay",
            enabled = true,
        )
        assertEquals("set_network_role", command.getString("command"))
        val payload = command.getJSONObject("payload")
        assertEquals("家庭网络", payload.getString("network_name"))
        assertEquals("relay", payload.getString("role"))
        assertTrue(payload.getBoolean("enabled"))

        val envelope = JSONObject(CommandJson.envelope(command, requestId = 7, idempotencyKey = "k-1"))
        assertEquals(1, envelope.getInt("schema_version"))
        assertEquals(7L, envelope.getJSONObject("request").getLong("request_id"))
        assertEquals(
            "set_network_role",
            envelope.getJSONObject("request").getJSONObject("command").getString("command"),
        )
        val encodedKey = envelope.getString("idempotency_key")
        assertTrue(
            "idempotency_key must match authoritative contract ^idem_[A-Za-z0-9_-]{43}$: $encodedKey",
            Regex("^idem_[A-Za-z0-9_-]{43}$").matches(encodedKey),
        )
        // Retrying the same logical key yields the exact same wire key.
        assertEquals(
            encodedKey,
            JSONObject(CommandJson.envelope(command, requestId = 8, idempotencyKey = "k-1"))
                .getString("idempotency_key"),
        )
        // Already-canonical keys pass through untouched.
        assertEquals(
            encodedKey,
            JSONObject(CommandJson.envelope(command, requestId = 9, idempotencyKey = encodedKey))
                .getString("idempotency_key"),
        )
        assertFalse(JSONObject(CommandJson.envelope(command, requestId = 1)).has("idempotency_key"))
    }

    @Test
    fun urlsKeepTheVersionedPrefixExactlyOnce() {
        assertEquals("http://127.0.0.1:6668/api/v1/commands", CommandJson.commandsUrl(config))
        assertEquals(
            "http://127.0.0.1:6668/api/v1/operations/op-9",
            CommandJson.operationUrl(config, "op-9"),
        )
        assertEquals(
            "http://127.0.0.1:6668/api/v1/commands",
            CommandJson.commandsUrl(NodeApiConfig("http://127.0.0.1:6668/")),
        )
    }

    @Test
    fun acceptedSubmissionIsInFlightNotSuccess() {
        val outcome = CommandResponseParser.parseCommandResponse(
            """{"schema_version":1,"request_id":1,"server_time":"t","operation":{"operation_id":"op-1","state":"queued","stage":"queued"}}""",
        )
        assertTrue(outcome is CommandOutcome.InProgress)
        assertFalse(outcome.isTerminalSuccess)
        assertEquals("op-1", (outcome as CommandOutcome.InProgress).operationId)

        val state = NodeCommandBridge.toState(outcome)
        assertTrue(state is CommandState.InFlight)
        assertFalse(state.isSettled)
    }

    @Test
    fun terminalOperationStatesMapToSuccessAndRejection() {
        val succeeded = CommandResponseParser.parseOperation(
            """{"operation":{"operation_id":"op-1","state":"succeeded","stage":"applied","result":null}}""",
        )
        assertTrue(succeeded is CommandOutcome.Succeeded)
        assertEquals("applied", (succeeded as CommandOutcome.Succeeded).stage)

        val failed = CommandResponseParser.parseOperation(
            """{"operation":{"operation_id":"op-2","state":"failed","stage":"applying","error":{"code":"node.error.role_unavailable","message_key":"state.error.detail","retryable":true}}}""",
        )
        assertTrue(failed is CommandOutcome.Rejected)
        failed as CommandOutcome.Rejected
        assertEquals("node.error.role_unavailable", failed.problemCode)
        assertTrue(failed.retryable)
    }

    @Test
    fun timeoutAndCancelledOperationsAreNotSuccess() {
        for (state in listOf("timed_out", "cancelled")) {
            val outcome = CommandResponseParser.parseOperation(
                """{"operation":{"operation_id":"op-3","state":"$state","stage":"applying"}}""",
            )
            assertTrue(outcome is CommandOutcome.Rejected)
        }
    }

    @Test
    fun rejectedBodyKeepsStableProblemCodeAndSafeMessageKey() {
        val rejected = CommandResponseParser.parseProblem(
            """{"schema_version":1,"request_id":2,"server_time":"t","problem":{"code":"node.error.role_unavailable","message_key":"state.unsupported.detail","retryable":false}}""",
        )
        assertEquals("node.error.role_unavailable", rejected.problemCode)
        assertEquals("state.unsupported.detail", rejected.messageKey)
        assertFalse(rejected.retryable)

        // Prose is not a localization key: fall back to an existing surface key.
        val prose = CommandResponseParser.parseProblem(
            """{"problem":{"code":"node.error.invalid_request","message":"network name must be non-empty"}}""",
        )
        assertEquals("state.error.detail", prose.messageKey)
    }

    @Test
    fun aBodyWithoutOperationOrResultIsNeverASuccess() {
        val outcome = CommandResponseParser.parseCommandResponse("""{"schema_version":1,"request_id":3}""")
        assertTrue(outcome is CommandOutcome.Failed)
        assertEquals(BridgeFailure.Malformed, (outcome as CommandOutcome.Failed).failure)

        assertTrue(CommandResponseParser.parseOperation("not json") is CommandOutcome.Failed)
        assertTrue(CommandResponseParser.parseCommandResponse("[]") is CommandOutcome.Failed)
    }

    @Test
    fun bridgeFollowsAnOperationToItsTerminalState() {
        var operationPolls = 0
        val transport = object : CommandTransport {
            override fun submit(body: String): CommandOutcome =
                CommandOutcome.InProgress("op-1", "queued", "queued")

            override fun operation(operationId: String): CommandOutcome {
                operationPolls += 1
                return if (operationPolls < 2) {
                    CommandOutcome.InProgress(operationId, "running", "applying")
                } else {
                    CommandOutcome.Succeeded(operationId, "applied")
                }
            }
        }
        val bridge = NodeCommandBridge(transport, pollIntervalMillis = 1, maxPolls = 10)
        val scope = CoroutineScope(Dispatchers.Default)
        runBlocking {
            withTimeout(5_000) {
                bridge.submit(scope, "{}").join()
            }
        }
        assertEquals(2, operationPolls)
        assertEquals(CommandState.Succeeded("applied"), bridge.state().value)
    }

    @Test
    fun bridgeStopsPollingWhenTheOperationNeverFinishes() {
        val transport = object : CommandTransport {
            override fun submit(body: String): CommandOutcome =
                CommandOutcome.InProgress("op-1", "queued", "queued")

            override fun operation(operationId: String): CommandOutcome =
                CommandOutcome.InProgress(operationId, "running", "applying")
        }
        val bridge = NodeCommandBridge(transport, pollIntervalMillis = 1, maxPolls = 3)
        val scope = CoroutineScope(Dispatchers.Default)
        runBlocking {
            withTimeout(5_000) {
                bridge.submit(scope, "{}").join()
            }
        }
        val state = bridge.state().value
        assertTrue(state is CommandState.Rejected)
        assertEquals("node.error.operation_timeout", (state as CommandState.Rejected).problemCode)
    }
}
