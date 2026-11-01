package cc.xlei.fuckipv6.contract;

import java.util.Collections;

import cc.xlei.fuckipv6.contract.NodeContractV1.CommandEnvelope;
import cc.xlei.fuckipv6.contract.NodeContractV1.CapabilityState;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCleanupFailureReason;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCleanupStepStatus;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeEventReplayReason;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeProblemCode;
import cc.xlei.fuckipv6.contract.NodeContractV1.OperationState;

/** Dependency-free executable checks for Android-safe contract bindings. */
public final class NodeContractV1Test {
    private interface FailingCall {
        void run();
    }

    private static void assertTrue(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }

    private static void assertSame(Object expected, Object actual, String message) {
        if (expected != actual) {
            throw new AssertionError(message + ": expected=" + expected + " actual=" + actual);
        }
    }

    private static void expectIllegalArgument(FailingCall call, String label) {
        try {
            call.run();
        } catch (IllegalArgumentException expected) {
            return;
        }
        throw new AssertionError(label + " must fail closed");
    }

    public static void main(String[] args) throws Exception {
        assertTrue(
                NodeContractV1.DTO_CLASS_NAMES.length == 79,
                "generated DTO inventory changed without updating the contract test");
        for (String name : NodeContractV1.DTO_CLASS_NAMES) {
            Class.forName("cc.xlei.fuckipv6.contract.NodeContractV1$" + name);
        }

        for (String field : new String[] {
            "actor",
            "attempt",
            "created_at_unix_ms",
            "error",
            "idempotency_key",
            "kind",
            "max_attempts",
            "operation_id",
            "result",
            "stage",
            "state",
            "target",
            "updated_at_unix_ms"
        }) {
            NodeContractV1.NodeOperationSnapshot.class.getDeclaredField(field);
        }
        NodeContractV1.NodeApiOperationResponse.class.getDeclaredField("operation");
        NodeContractV1.NodeApiCommandResponse.class.getDeclaredField("operation");
        NodeContractV1.NodeCommandHttpRequest.class.getDeclaredField("idempotency_key");
        for (String field : new String[] {"kind", "sequence", "snapshot_revision"}) {
            NodeContractV1.NodeEvent.class.getDeclaredField(field);
        }
        for (String field : new String[] {"sequence", "snapshot_revision"}) {
            NodeContractV1.NodeEventCursor.class.getDeclaredField(field);
        }
        NodeContractV1.NodeApiEvent.Snapshot.class.getDeclaredField("cursor");
        for (String field : new String[] {
            "latest_cursor", "reason", "requested_cursor"
        }) {
            NodeContractV1.NodeApiEvent.ResyncRequired.class.getDeclaredField(field);
        }
        for (String field : new String[] {
            "latest_cursor", "requested_cursor", "resync_reason"
        }) {
            NodeContractV1.NodeApiErrorResponse.class.getDeclaredField(field);
        }

        for (String field : new String[] {
            "revision",
            "lifecycle",
            "identity",
            "configuration",
            "exit_node_capabilities",
            "capabilities",
            "permissions",
            "embedded_services",
            "ui_mode",
            "networks",
            "peers",
            "recent_activity",
            "logs",
            "last_error"
        }) {
            NodeContractV1.NodeSnapshot.class.getDeclaredField(field);
        }

        assertSame(
                NodeCommandKind.GET_SNAPSHOT,
                NodeCommandKind.fromWireName("get_snapshot"),
                "known command");
        assertSame(
                NodeCommandKind.GET_NODE_DIAGNOSTICS,
                NodeCommandKind.fromWireName("get_node_diagnostics"),
                "runtime diagnostics command");
        assertSame(
                NodeCommandKind.CLEANUP_NODE_RESIDUALS,
                NodeCommandKind.fromWireName("cleanup_node_residuals"),
                "runtime cleanup command");
        assertSame(
                NodeCommandKind.GET_CONFIGURATION,
                NodeCommandKind.fromWireName("get_configuration"),
                "configuration read command");
        assertSame(
                NodeCommandKind.PREVIEW_CONFIGURATION,
                NodeCommandKind.fromWireName("preview_configuration"),
                "configuration preview command");
        assertSame(
                NodeCommandKind.APPLY_CONFIGURATION,
                NodeCommandKind.fromWireName("apply_configuration"),
                "configuration apply command");
        assertSame(
                NodeCommandKind.ROLLBACK_CONFIGURATION,
                NodeCommandKind.fromWireName("rollback_configuration"),
                "configuration rollback command");
        assertSame(
                NodeCommandKind.GET_CONFIGURATION_HISTORY,
                NodeCommandKind.fromWireName("get_configuration_history"),
                "configuration history command");
        expectIllegalArgument(
                () -> NodeCommandKind.fromWireName("not_a_command"),
                "unknown command");
        expectIllegalArgument(() -> NodeCommandKind.fromWireName(null), "null command");
        assertSame(
                CapabilityState.UNKNOWN,
                CapabilityState.fromWireName("unknown"),
                "migrated capability state");
        assertSame(
                NodeProblemCode.UNSUPPORTED,
                NodeProblemCode.fromWireName("unsupported"),
                "platform-unsupported problem code");
        assertSame(
                NodeCleanupFailureReason.OWNER_UNAVAILABLE,
                NodeCleanupFailureReason.fromWireName("owner_unavailable"),
                "runtime cleanup failure reason");
        assertSame(
                NodeCleanupStepStatus.DEFERRED,
                NodeCleanupStepStatus.fromWireName("deferred"),
                "runtime cleanup step status");
        assertSame(
                NodeEventReplayReason.REVISION_GAP,
                NodeEventReplayReason.fromWireName("revision_gap"),
                "event replay reason");
        assertSame(
                OperationState.TIMED_OUT,
                OperationState.fromWireName("timed_out"),
                "operation timed-out state");
        expectIllegalArgument(
                () -> OperationState.fromWireName("invented"),
                "unknown operation state");
        expectIllegalArgument(
                () -> CapabilityState.fromWireName("invented"),
                "unknown capability state");
        expectIllegalArgument(
                () -> NodeProblemCode.fromWireName(null),
                "null problem code");

        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.CREATE_NETWORK),
                "create_network must be a visible action");
        assertTrue(
                !NodeContractV1.isManifestAction(NodeCommandKind.GET_UI_MANIFEST),
                "get_ui_manifest must remain bootstrap-only");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.GET_NODE_DIAGNOSTICS),
                "get_node_diagnostics must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.CLEANUP_NODE_RESIDUALS),
                "cleanup_node_residuals must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.GET_CONFIGURATION),
                "get_configuration must be a visible action");
        assertTrue(
                NodeContractV1.isManifestAction(NodeCommandKind.APPLY_CONFIGURATION),
                "apply_configuration must be a visible action");

        CommandEnvelope envelope = NodeContractV1.commandEnvelope(1, 7, "get_snapshot");
        assertTrue(envelope.requestId == 7, "request id must be preserved");
        assertSame(NodeCommandKind.GET_SNAPSHOT, envelope.command, "command identity");
        assertSame(
                NodeCommandKind.GET_NODE_DIAGNOSTICS,
                NodeContractV1.commandEnvelope(1, 8, "get_node_diagnostics").command,
                "runtime diagnostics envelope");
        assertSame(
                NodeCommandKind.CLEANUP_NODE_RESIDUALS,
                NodeContractV1.commandEnvelope(1, 9, "cleanup_node_residuals").command,
                "runtime cleanup envelope");

        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(2, 1, "get_snapshot"),
                "unsupported schema version");
        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(1, -1, "get_snapshot"),
                "negative request id");
        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(1, 1, "get_ui_manifest"),
                "bootstrap command in an action envelope");
        expectIllegalArgument(
                () -> NodeContractV1.commandEnvelope(1, 1, "not_a_command"),
                "unknown envelope command");

        for (NodeCommandKind command : NodeCommandKind.values()) {
            boolean expected = command != NodeCommandKind.GET_UI_MANIFEST;
            assertTrue(
                    NodeContractV1.isManifestAction(command) == expected,
                    "unexpected manifest action classification for " + command.wireName());
        }

        NodeContractV1.NodeCommand.GetSnapshot getSnapshot =
                new NodeContractV1.NodeCommand.GetSnapshot();
        assertSame(
                NodeCommandKind.GET_SNAPSHOT,
                getSnapshot.command,
                "get_snapshot command DTO");
        NodeContractV1.NodeCommand.GetNodeDiagnostics getDiagnostics =
                new NodeContractV1.NodeCommand.GetNodeDiagnostics();
        assertSame(
                NodeCommandKind.GET_NODE_DIAGNOSTICS,
                getDiagnostics.command,
                "get_node_diagnostics command DTO");
        NodeContractV1.NodeCommand.CleanupNodeResiduals cleanup =
                new NodeContractV1.NodeCommand.CleanupNodeResiduals(
                        new NodeContractV1.NodeCommand.CleanupNodeResiduals.CleanupNodeResidualsPayload(
                                false, Collections.emptyList()));
        assertSame(
                NodeCommandKind.CLEANUP_NODE_RESIDUALS,
                cleanup.command,
                "cleanup_node_residuals command DTO");
        NodeContractV1.NodeCreateNetworkCommand createNetwork =
                new NodeContractV1.NodeCreateNetworkCommand(true, null, "lab", 41000, 41001);
        NodeContractV1.NodeCommand.CreateNetwork createCommand =
                new NodeContractV1.NodeCommand.CreateNetwork(createNetwork);
        assertSame(
                NodeCommandKind.CREATE_NETWORK,
                createCommand.command,
                "create_network command DTO");
        assertSame(createNetwork, createCommand.payload, "create_network payload identity");

        NodeContractV1.NodeCommandOutcome.Completed completed =
                new NodeContractV1.NodeCommandOutcome.Completed(
                        new NodeContractV1.NodeCommandOutcome.Completed.CompletedData("done"));
        assertSame("completed", completed.status, "completed outcome tag");
        assertSame("done", completed.data.message, "completed outcome payload");

        new NodeContractV1.NodeCommandOutcome.Snapshot(null);
        new NodeContractV1.NodeCommandOutcome.UiManifest(null);
        new NodeContractV1.NodeCommandOutcome.Diagnostics(null);
        new NodeContractV1.NodeCommandOutcome.Cleanup(null);
        new NodeContractV1.NodeCommandOutcome.Probe(null);
        new NodeContractV1.NodeCommandOutcome.Configuration(null);
        new NodeContractV1.NodeCommandOutcome.ConfigurationPreview(null);
        new NodeContractV1.NodeCommandOutcome.ConfigurationApplied(null);
        new NodeContractV1.NodeCommandOutcome.ConfigurationHistory(null);
        new NodeContractV1.NodeCommandOutcome.Accepted(null);
        new NodeContractV1.NodeCommandOutcome.Rejected(null);

        new NodeContractV1.NodeEventKind.StateChanged(null);
        new NodeContractV1.NodeEventKind.NetworkChanged(null);
        new NodeContractV1.NodeEventKind.EmbeddedServiceChanged(null);
        new NodeContractV1.NodeEventKind.LogAppended(null);
        new NodeContractV1.NodeEventKind.RoutingChanged(null);
        new NodeContractV1.NodeEventKind.RoleChanged(null);
        new NodeContractV1.NodeEventKind.CommandCompleted(null);

        assertSame(
                "default", new NodeContractV1.NodeNetworkSelection.Default().kind,
                "default network selection tag");
        assertSame(
                "all", new NodeContractV1.NodeNetworkSelection.All().kind,
                "all network selection tag");
        assertSame(
                "none", new NodeContractV1.NodeNetworkSelection.None().kind,
                "none network selection tag");
        assertSame(
                "named", new NodeContractV1.NodeNetworkSelection.Named(null).kind,
                "named network selection tag");

        new NodeContractV1.NodeWsMessage.Snapshot(1, null);
        new NodeContractV1.NodeWsMessage.Event(null, 1);
        new NodeContractV1.NodeWsMessage.Resync(null, 1, null);
        new NodeContractV1.NodeWsMessage.Error(
                null, null, "error", "error.operation_internal", null, null, null, false, 1);

        System.out.println("NodeContractV1Test: passed");
    }

    private NodeContractV1Test() {}
}
