package cc.xlei.fuckipv6.contract;

import cc.xlei.fuckipv6.contract.NodeContractV1.CommandEnvelope;
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind;

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

    public static void main(String[] args) {
        assertSame(
                NodeCommandKind.GET_SNAPSHOT,
                NodeCommandKind.fromWireName("get_snapshot"),
                "known command");
        expectIllegalArgument(
                () -> NodeCommandKind.fromWireName("not_a_command"),
                "unknown command");
        expectIllegalArgument(() -> NodeCommandKind.fromWireName(null), "null command");

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

        System.out.println("NodeContractV1Test: passed");
    }

    private NodeContractV1Test() {}
}
