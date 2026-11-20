package cc.xlei.fuckipv6.android.bridge

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/**
 * Exercises the production HTTP clients against a loopback socket server.
 *
 * Method, headers, bearer token, status mapping, streaming and SSE framing are
 * only proven by a real round trip; the fakes cover the state machines, not the
 * transport.
 */
class HttpBridgeIntegrationTest {
    private var server: TestHttpServer? = null
    private var baseUrl = ""
    private var forceUnauthorized = false

    private val snapshotJson = """
        {
          "schema_version": 1,
          "request_id": 1,
          "server_time": "2026-11-21T00:00:00Z",
          "snapshot": {
            "revision": 7,
            "file_capability": {
              "state": "enabled",
              "network_id": "net-1",
              "access_epoch": 2,
              "share_revision": 4,
              "available_bytes": 1073741824,
              "active_transfers": 1
            },
            "memberships": [{"network_id": "net-1", "network_name": "家庭网络"}],
            "networks": [{"name": "家庭网络", "roles": ["member", "relay"]}],
            "embedded_services": [
              {
                "network_name": "家庭网络",
                "relay_health": {
                  "status": "healthy",
                  "lifecycle": "running",
                  "network_reachability": "reachable",
                  "listener_bound": true,
                  "forwarding_observed": true,
                  "authorization_valid": true,
                  "issues": []
                }
              }
            ],
            "permissions": {"access_model": "trusted_control_plane", "permissions": ["manage_roles"]}
          }
        }
    """.trimIndent()

    @Before
    fun startServer() {
        val started = TestHttpServer { request ->
            when {
                forceUnauthorized -> TestHttpServer.Response(401, "")
                request.path == "/api/v1/snapshot" ->
                    TestHttpServer.Response(200, snapshotJson)
                request.path == "/api/v1/commands" ->
                    TestHttpServer.Response(
                        200,
                        """{"schema_version":1,"request_id":11,"server_time":"t","operation":{"operation_id":"op-1","state":"queued","stage":"queued"}}""",
                    )
                request.path == "/api/v1/operations/op-1" ->
                    TestHttpServer.Response(
                        200,
                        """{"operation":{"operation_id":"op-1","state":"succeeded","stage":"applied","result":{"outcome":{"status":"set_network_role","data":{"ok":true}}}}}""",
                    )
                request.path == "/api/v1/file/targets" ->
                    TestHttpServer.Response(
                        200,
                        """[{"peer":"peer-a","network_id":"net-1","network_name":"家庭网络","share_id":"share-1"}]""",
                    )
                request.path == "/api/v1/file/directory" ->
                    TestHttpServer.Response(
                        200,
                        """{"entries":[{"path":"docs/a.md","is_directory":false,"size":12}],"truncated":false}""",
                    )
                request.path == "/api/v1/events" ->
                    TestHttpServer.Response(
                        200,
                        "data: {\"type\":\"snapshot\",\"cursor\":{\"sequence\":3,\"snapshot_revision\":7},\"snapshot\":{\"revision\":7}}\n\n",
                        "text/event-stream",
                    )
                else -> TestHttpServer.Response(404, "{}")
            }
        }
        server = started
        baseUrl = "http://127.0.0.1:${started.port}"
    }

    @After
    fun stopServer() {
        server?.close()
    }

    private fun config(token: String? = null) = NodeApiConfig(baseUrl, token)

    @Test
    fun snapshotRoundTripCarriesTheBearerTokenAndEverySlice() {
        val bridge = NodeBridge(HttpSnapshotSource(config("token-1")))
        bridge.pollOnce()
        val request = server!!.requests.single()
        assertEquals("GET", request.method)
        assertEquals("/api/v1/snapshot", request.path)
        assertEquals("Bearer token-1", request.headers["authorization"])

        val state = bridge.state().value
        assertTrue(state is BridgeState.Ready)
        state as BridgeState.Ready
        assertEquals(7L, state.snapshotRevision)
        assertEquals("status.capability.enabled", state.data.stateKey)
        assertEquals("家庭网络", state.data.networkLabel)
        assertEquals("1.0 GiB", state.data.formattedAvailable)
        assertEquals("家庭网络", state.relay?.networkName)
        assertTrue(state.relay?.health?.listener_bound == true)
        assertEquals(
            setOf(cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission.MANAGE_ROLES),
            state.relay?.permissions,
        )
        assertEquals("net-1", state.networks.single().networkId)
    }

    @Test
    fun unauthorizedSnapshotIsAFailureValueWithoutRetry() {
        forceUnauthorized = true
        val bridge = NodeBridge(HttpSnapshotSource(config()))
        bridge.pollOnce()
        val state = bridge.state().value
        assertTrue(state is BridgeState.Failed)
        assertEquals("bridge.error.unauthorized", (state as BridgeState.Failed).failure.key)
        assertEquals(1, server!!.requests.size)
    }

    @Test
    fun commandRoundTripPostsTheEnvelopeAndFollowsTheOperation() {
        val bridge = NodeCommandBridge(NodeCommandClient(config("token-2")), pollIntervalMillis = 1)
        val scope = CoroutineScope(Dispatchers.Default)
        val body = CommandJson.envelope(
            CommandJson.setNetworkRole("家庭网络", "relay", true),
            requestId = 11,
        )
        runBlocking { withTimeout(5_000) { bridge.submit(scope, body).join() } }

        val posted = server!!.requests.first { it.path == "/api/v1/commands" }
        assertEquals("POST", posted.method)
        assertTrue(posted.body.contains("\"set_network_role\""))
        assertEquals("Bearer token-2", posted.headers["authorization"])
        assertEquals(CommandState.Succeeded("applied"), bridge.state().value)

        val followed = server!!.requests.first { it.path.startsWith("/api/v1/operations/") }
        assertEquals("GET", followed.method)
    }

    @Test
    fun browseRoundTripReturnsTargetsAndDirectoryEntries() {
        val client = FileBrowseClient(config())
        val targets = client.targets()
        assertTrue(targets is BrowseResult.Ok)
        assertEquals("peer-a", (targets as BrowseResult.Ok).value.single().peer)

        val page = client.directory(BrowseRequest("peer-a", "share-1", "net-1", "docs"))
        assertTrue(page is BrowseResult.Ok)
        val directory = (page as BrowseResult.Ok).value
        assertEquals("docs/a.md", directory.entries.single().path)
        assertEquals(12L, directory.entries.single().size)
        assertNull(directory.nextCursor)

        val directoryRequest = server!!.requests.first { it.path == "/api/v1/file/directory" }
        assertTrue(directoryRequest.query.contains("peer=peer-a"))
        assertTrue(directoryRequest.query.contains("path=docs"))
        assertTrue(directoryRequest.query.contains("network_id=net-1"))
    }

    @Test
    fun eventStreamDeliversAResumableSnapshotFrame() {
        val frames = mutableListOf<NodeEventFrame>()
        val failure = NodeEventStream(config("token-3"), resumeCursor = "3:7").read { frames += it }
        assertNull(failure)
        val request = server!!.requests.single()
        assertEquals("/api/v1/events", request.path)
        assertEquals("after=3%3A7", request.query)
        assertEquals("3:7", request.headers["last-event-id"])
        assertEquals("text/event-stream", request.headers["accept"])

        val frame = frames.single()
        assertEquals("snapshot", frame.type)
        assertEquals("3:7", frame.resumeToken())
        assertTrue(frame.action() is LiveAction.ApplySnapshot)
    }

    @Test
    fun connectionRefusedIsAFailureValueNotAnException() {
        // Port 1 has no listener without privileges. Another test may still race
        // to an ephemeral port this class just released, so only "a failure, and
        // not a fabricated empty list" is asserted.
        val result = FileBrowseClient(NodeApiConfig("http://127.0.0.1:1")).targets()
        assertTrue(result is BrowseResult.Failed)
        val failure = (result as BrowseResult.Failed).failure
        assertTrue(
            "unexpected failure: $failure",
            failure == BridgeFailure.Unreachable ||
                failure is BridgeFailure.HttpStatus ||
                failure is BridgeFailure.Unauthorized,
        )
    }
}
