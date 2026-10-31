package cc.xlei.fuckipv6.contract;

import cc.xlei.fuckipv6.contract.NodeContractV1.CommandEnvelope;
import cc.xlei.fuckipv6.contract.NodeContractV1.CapabilityState;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeProblemCode;

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
                NodeContractV1.DTO_CLASS_NAMES.length == 42,
                "generated DTO inventory changed without updating the contract test");
        for (String name : NodeContractV1.DTO_CLASS_NAMES) {
            Class.forName("cc.xlei.fuckipv6.contract.NodeContractV1$" + name);
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
            "logs",
            "last_error"
        }) {
            NodeContractV1.NodeSnapshot.class.getDeclaredField(field);
        }

        assertSame(
                NodeCommandKind.GET_SNAPSHOT,
                NodeCommandKind.fromWireName("get_snapshot"),
                "known command");
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

        CommandEnvelope envelope = NodeContractV1.commandEnvelope(1, 7, "get_snapshot");
        assertTrue(envelope.requestId == 7, "request id must be preserved");
        assertSame(NodeCommandKind.GET_SNAPSHOT, envelope.command, "command identity");

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
        NodeContractV1.NodeCreateNetworkCommand createNetwork =
                new NodeContractV1.NodeCreateNetworkCommand("lab", 41000, 41001, null, true);
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
        new NodeContractV1.NodeCommandOutcome.Probe(null);
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
        new NodeContractV1.NodeWsMessage.Event(1, null);
        new NodeContractV1.NodeWsMessage.Resync(1, null, null);
        new NodeContractV1.NodeWsMessage.Error(1, null, "error");

        System.out.println("NodeContractV1Test: passed");
    }

    private NodeContractV1Test() {}
}
