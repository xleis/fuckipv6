import XCTest

final class FakeNode: NodeControlling, @unchecked Sendable {
    var phase: NodePhase? = .running
    var snapshot: Data = Data(#"{"state":"running","network":"mesh"}"#.utf8)
    var snapshotError: Fv6Failure?
    var outcome: Data = Data(#"{"applied":true}"#.utf8)
    var outcomeError: Fv6Failure?
    var pathChangeError: Fv6Failure?
    var lastErrorText = ""
    private(set) var executeCalls: [Data] = []
    private(set) var pathChangeCalls = 0

    func snapshotJSON(timeout: TimeInterval) throws -> Data {
        if let snapshotError {
            throw snapshotError
        }
        return snapshot
    }

    func execute(requestJSON: Data, timeout: TimeInterval) throws -> Data {
        executeCalls.append(requestJSON)
        if let outcomeError {
            throw outcomeError
        }
        return outcome
    }

    func networkPathChanged(timeout: TimeInterval) throws {
        pathChangeCalls += 1
        if let pathChangeError {
            throw pathChangeError
        }
    }

    func lastError() -> String {
        lastErrorText
    }
}

final class MessageRouterTests: XCTestCase {
    private var node: FakeNode?
    private var router: MessageRouter!

    override func setUp() {
        super.setUp()
        node = FakeNode()
        router = makeRouter()
    }

    private func makeRouter() -> MessageRouter {
        let node = self.node
        return MessageRouter { node }
    }

    private func reply(_ payload: String) -> [String: Any] {
        let data = router.handle(Data(payload.utf8))
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            XCTFail("reply is not a JSON object")
            return [:]
        }
        return object
    }

    private func errorCode(_ object: [String: Any]) -> Int? {
        (object["error"] as? [String: Any])?["code"] as? Int
    }

    func testRejectsEmptyAndOversizedMessages() {
        XCTAssertEqual(errorCode(reply("")), Int(Fv6Status.invalidArgument.rawValue))

        let oversized = String(repeating: "a", count: MessageRouter.maximumMessageBytes + 1)
        XCTAssertEqual(errorCode(reply(oversized)), Int(Fv6Status.invalidArgument.rawValue))
        XCTAssertEqual(reply("not json")["ok"] as? Bool, false)
    }

    func testRejectsUnsupportedVersionAndUnknownCommand() {
        XCTAssertEqual(errorCode(reply(#"{"v":2,"cmd":"status"}"#)), Int(Fv6Status.invalidArgument.rawValue))
        XCTAssertEqual(errorCode(reply(#"{"v":1,"cmd":"shutdown"}"#)), Int(Fv6Status.invalidArgument.rawValue))
    }

    func testRequiresARunningNode() {
        node = nil
        router = makeRouter()

        let object = reply(#"{"v":1,"cmd":"snapshot"}"#)
        XCTAssertEqual(object["ok"] as? Bool, false)
        XCTAssertEqual(errorCode(object), Int(Fv6Status.state.rawValue))
        XCTAssertEqual((object["error"] as? [String: Any])?["message"] as? String, "node is not running")
    }

    func testStatusReportsPhaseAndLastError() {
        node?.phase = .running
        node?.lastErrorText = ""

        let object = reply(#"{"v":1,"cmd":"status"}"#)
        XCTAssertEqual(object["ok"] as? Bool, true)
        XCTAssertEqual(object["v"] as? Int, MessageRouter.version)
        XCTAssertEqual(object["cmd"] as? String, "status")
        let result = object["result"] as? [String: Any]
        XCTAssertEqual(result?["phase"] as? Int, Int(NodePhase.running.rawValue))
        XCTAssertEqual(result?["lastError"] as? String, "")
    }

    func testStatusForANodeWithoutPhaseUsesMinusOne() {
        node?.phase = nil
        let result = reply(#"{"v":1,"cmd":"status"}"#)["result"] as? [String: Any]
        XCTAssertEqual(result?["phase"] as? Int, -1)
    }

    func testSnapshotIsForwardedAsAnObject() {
        let object = reply(#"{"v":1,"cmd":"snapshot"}"#)
        XCTAssertEqual(object["ok"] as? Bool, true)
        let result = object["result"] as? [String: Any]
        XCTAssertEqual(result?["network"] as? String, "mesh")
        XCTAssertEqual(result?["state"] as? String, "running")
    }

    func testNonJSONSnapshotIsRejected() {
        node?.snapshot = Data("running".utf8)
        XCTAssertEqual(errorCode(reply(#"{"v":1,"cmd":"snapshot"}"#)), Int(Fv6Status.invalidArgument.rawValue))
    }

    func testExecuteForwardsTheRequestAndReturnsTheOutcome() {
        let object = reply(#"{"v":1,"cmd":"execute","request":"{\"kind\":\"status\"}"}"#)
        XCTAssertEqual(object["ok"] as? Bool, true)
        XCTAssertEqual((object["result"] as? [String: Any])?["applied"] as? Bool, true)
        XCTAssertEqual(node?.executeCalls.count, 1)
        XCTAssertEqual(
            node?.executeCalls.first.map { String(decoding: $0, as: UTF8.self) },
            #"{"kind":"status"}"#
        )
    }

    func testExecuteRequiresAJSONObjectRequest() {
        XCTAssertEqual(
            errorCode(reply(#"{"v":1,"cmd":"execute"}"#)),
            Int(Fv6Status.invalidArgument.rawValue)
        )
        XCTAssertEqual(
            errorCode(reply(#"{"v":1,"cmd":"execute","request":"nope"}"#)),
            Int(Fv6Status.invalidArgument.rawValue)
        )
        XCTAssertEqual(node?.executeCalls.count, 0)
    }

    func testNodeFailuresKeepTheirCode() {
        node?.snapshotError = Fv6Failure(status: .timeout, message: "snapshot timed out")
        XCTAssertEqual(errorCode(reply(#"{"v":1,"cmd":"snapshot"}"#)), Int(Fv6Status.timeout.rawValue))

        node?.outcomeError = Fv6Failure(status: .nodeProblem, message: "node rejected")
        let object = reply(#"{"v":1,"cmd":"execute","request":"{\"kind\":\"stop\"}"}"#)
        XCTAssertEqual(errorCode(object), Int(Fv6Status.nodeProblem.rawValue))
        XCTAssertEqual((object["error"] as? [String: Any])?["message"] as? String, "node rejected")
    }

    func testPathChangedInvokesTheNode() {
        XCTAssertEqual(reply(#"{"v":1,"cmd":"pathChanged"}"#)["ok"] as? Bool, true)
        XCTAssertEqual(node?.pathChangeCalls, 1)

        node?.pathChangeError = Fv6Failure(status: .busy, message: "operation in progress")
        XCTAssertEqual(errorCode(reply(#"{"v":1,"cmd":"pathChanged"}"#)), Int(Fv6Status.busy.rawValue))
    }
}
