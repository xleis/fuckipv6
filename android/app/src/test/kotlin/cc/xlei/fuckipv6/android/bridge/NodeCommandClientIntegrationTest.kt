package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Real-socket tests for the command transport.
 *
 * The client builds URLs, headers and method selection itself, so only an actual
 * request proves them; a fake transport would let a wrong path or a missing
 * bearer token pass unnoticed.
 */
class NodeCommandClientIntegrationTest {
    private fun client(server: TestHttpServer, token: String? = "secret") =
        NodeCommandClient(NodeApiConfig(baseUrl = "http://127.0.0.1:${server.port}", token = token))

    @Test
    fun submissionPostsTheEnvelopeWithBearerAndJsonContentType() {
        val operation = """
            {"operation":{"operation_id":"op-1","state":"running","stage":"executing"}}
        """.trimIndent()
        TestHttpServer { _ -> TestHttpServer.Response(status = 200, body = operation) }.use { server ->
            val body = CommandJson.envelope(
                command = CommandJson.setNetworkRole("家庭网络", "relay", true),
                requestId = 7,
            )
            val outcome = client(server).submit(body)
            assertTrue(outcome is CommandOutcome.InProgress)
            val inFlight = outcome as CommandOutcome.InProgress
            assertEquals("op-1", inFlight.operationId)
            assertEquals("executing", inFlight.stage)

            val request = server.requests.single()
            assertEquals("POST", request.method)
            assertEquals("/api/v1/commands", request.path)
            assertEquals("Bearer secret", request.headers["authorization"])
            assertEquals("application/json", request.headers["content-type"])
            assertTrue(request.body.contains("\"request_id\":7"))
            assertTrue(request.body.contains("set_network_role"))
        }
    }

    @Test
    fun aDirectResultOutcomeIsSuccessOnlyWithATerminalStatus() {
        val body = """
            {"result":{"outcome":{"status":"join_requests_reviewed","data":{"network_id":"net-1"}}}}
        """.trimIndent()
        TestHttpServer { _ -> TestHttpServer.Response(status = 200, body = body) }.use { server ->
            val outcome = client(server).submit(CommandJson.envelope(snapshotCommand(), 1))
            val success = outcome as CommandOutcome.Succeeded
            assertEquals("join_requests_reviewed", success.outcomeStatus)
            assertEquals("net-1", success.data?.getString("network_id"))
            assertTrue(success.isTerminalSuccess)
        }
    }

    @Test
    fun operationPollRestoresTheOperationPathAndTerminalFailure() {
        val body = """
            {"operation":{"operation_id":"op-9","state":"failed","error":{"code":"node.error.routing_rejected","message_key":"state.error.detail","retryable":false}}}
        """.trimIndent()
        TestHttpServer { _ -> TestHttpServer.Response(status = 200, body = body) }.use { server ->
            val outcome = client(server).operation("op-9")
            val rejected = outcome as CommandOutcome.Rejected
            assertEquals("node.error.routing_rejected", rejected.problemCode)
            assertFalse(rejected.retryable)
            assertEquals("/api/v1/operations/op-9", server.requests.single().path)
            assertEquals("GET", server.requests.single().method)
        }
    }

    @Test
    fun clientProblemsAndMissingOperationsStayDistinct() {
        val problem = """{"problem":{"code":"join_request.error.batch_empty","message_key":"state.error.detail"}}"""
        TestHttpServer { _ -> TestHttpServer.Response(status = 422, body = problem) }.use { server ->
            val outcome = client(server).submit(CommandJson.envelope(snapshotCommand(), 1))
            assertEquals(
                "join_request.error.batch_empty",
                (outcome as CommandOutcome.Rejected).problemCode,
            )
        }
        TestHttpServer { _ -> TestHttpServer.Response(status = 404, body = "{}") }.use { server ->
            val outcome = client(server).operation("missing")
            assertEquals(
                "node.error.operation_not_found",
                (outcome as CommandOutcome.Rejected).problemCode,
            )
        }
        TestHttpServer { _ -> TestHttpServer.Response(status = 403, body = "{}") }.use { server ->
            val outcome = client(server).submit(CommandJson.envelope(snapshotCommand(), 1))
            assertTrue((outcome as CommandOutcome.Failed).failure is BridgeFailure.Unauthorized)
        }
    }

    @Test
    fun a200WithoutOperationOrResultIsMalformedNotSuccess() {
        TestHttpServer { _ -> TestHttpServer.Response(status = 200, body = "{}") }.use { server ->
            val outcome = client(server).submit(CommandJson.envelope(snapshotCommand(), 1))
            assertEquals(BridgeFailure.Malformed, (outcome as CommandOutcome.Failed).failure)
        }
    }

    @Test
    fun bridgeStopsAtThePollBudgetInsteadOfInventingSuccess() {
        var polls = 0
        val transport = object : CommandTransport {
            override fun submit(body: String): CommandOutcome =
                CommandOutcome.InProgress("op-1", "running", "executing")

            override fun operation(operationId: String): CommandOutcome {
                polls += 1
                return CommandOutcome.InProgress(operationId, "running", "executing")
            }
        }
        val bridge = NodeCommandBridge(transport, pollIntervalMillis = 1, maxPolls = 3)
        val scope = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Default)
        val job = bridge.submit(scope, "{}")
        kotlinx.coroutines.runBlocking { job.join() }
        assertEquals(3, polls)
        val state = bridge.state().value as CommandState.Rejected
        assertEquals("node.error.operation_timeout", state.problemCode)
        assertNull(bridge.lastOutcome().value as? CommandOutcome.Succeeded)
    }

    private fun snapshotCommand() = org.json.JSONObject()
        .put("command", "get_snapshot")
        .put("payload", org.json.JSONObject())
}
