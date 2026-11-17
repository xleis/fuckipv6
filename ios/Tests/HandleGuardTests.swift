import XCTest

/// Deterministic races for the reference-counted FFI handle guard. Every case
/// uses explicit acquire/close ordering plus semaphores so a failure points at a
/// real invariant break, not at scheduler timing.
final class HandleGuardTests: XCTestCase {
    private func makeGuard() -> (HandleGuard, () -> Int) {
        let counter = Counter()
        let guardrail = HandleGuard { counter.increment() }
        return (guardrail, { counter.value })
    }

    func testFreesOnlyAfterTheLastUserLeaves() {
        let (guardrail, freed) = makeGuard()
        XCTAssertTrue(guardrail.acquire())
        XCTAssertTrue(guardrail.acquire())
        XCTAssertEqual(guardrail.userCount, 2)

        XCTAssertEqual(guardrail.close(), .deferred(users: 2))
        XCTAssertEqual(freed(), 0)

        guardrail.release()
        XCTAssertEqual(freed(), 0)
        XCTAssertEqual(guardrail.userCount, 1)

        guardrail.release()
        XCTAssertEqual(freed(), 1)
        XCTAssertEqual(guardrail.userCount, 0)
    }

    func testCloseWithoutUsersFreesImmediately() {
        let (guardrail, freed) = makeGuard()
        XCTAssertEqual(guardrail.close(), .freed)
        XCTAssertEqual(freed(), 1)
        XCTAssertTrue(guardrail.isClosed)
    }

    func testSecondCloseIsAlreadyClosedAndNeverFreesTwice() {
        let (guardrail, freed) = makeGuard()
        XCTAssertEqual(guardrail.close(), .freed)
        XCTAssertEqual(guardrail.close(), .alreadyClosed)
        XCTAssertEqual(freed(), 1)
    }

    func testAcquireFailsAfterClose() {
        let (guardrail, _) = makeGuard()
        XCTAssertEqual(guardrail.close(), .freed)
        XCTAssertFalse(guardrail.acquire())
        XCTAssertEqual(guardrail.userCount, 0)
    }

    func testCloseWhileUsersAreRunningDefersUntilTheyLeave() {
        let (guardrail, freed) = makeGuard()
        XCTAssertTrue(guardrail.acquire())

        let closed = expectation(description: "closer returned")
        let outcome = Box<HandleGuard.CloseOutcome?>(nil)
        Thread.detachNewThread {
            outcome.value = guardrail.close()
            closed.fulfill()
        }
        wait(for: [closed], timeout: 1)
        XCTAssertEqual(outcome.value, .deferred(users: 1))
        XCTAssertEqual(freed(), 0)
        XCTAssertFalse(guardrail.waitUntilFree(timeout: 0.05))

        guardrail.release()
        XCTAssertTrue(guardrail.waitUntilFree(timeout: 1))
        XCTAssertEqual(freed(), 1)
    }

    func testConcurrentClosersFreeExactlyOnce() {
        for _ in 0 ..< 8 {
            let (guardrail, freed) = makeGuard()
            XCTAssertTrue(guardrail.acquire())

            let group = DispatchGroup()
            let closers = 8
            for _ in 0 ..< closers {
                group.enter()
                Thread.detachNewThread {
                    _ = guardrail.close()
                    group.leave()
                }
            }
            XCTAssertEqual(group.wait(timeout: .now() + 2), .success)
            // Everyone closed before the last user left, so nothing is freed yet.
            XCTAssertEqual(freed(), 0)

            guardrail.release()
            XCTAssertEqual(freed(), 1)
            XCTAssertTrue(guardrail.isClosed)
        }
    }

    func testCloseWaitsForEveryUserThatAlreadyEntered() {
        let (guardrail, freed) = makeGuard()
        let entered = Counter()
        let group = DispatchGroup()
        let gate = DispatchSemaphore(value: 0)
        let users = 16

        for _ in 0 ..< users {
            group.enter()
            Thread.detachNewThread {
                guard guardrail.acquire() else {
                    group.leave()
                    return
                }
                entered.increment()
                gate.wait()
                guardrail.release()
                group.leave()
            }
        }
        XCTAssertTrue(waitUntil(2) { entered.value == users })
        XCTAssertEqual(guardrail.close(), .deferred(users: users))
        XCTAssertEqual(freed(), 0)

        for _ in 0 ..< users {
            gate.signal()
        }
        XCTAssertEqual(group.wait(timeout: .now() + 3), .success)
        XCTAssertEqual(freed(), 1)
        XCTAssertEqual(guardrail.userCount, 0)
    }

    func testReleaseWithoutAcquireDoesNotOverflowTheCount() {
        let (guardrail, freed) = makeGuard()
        XCTAssertEqual(guardrail.close(), .freed)
        guardrail.release()
        guardrail.release()
        XCTAssertEqual(guardrail.userCount, 0)
        XCTAssertEqual(freed(), 1)
    }
}

/// Small thread-safe helpers shared by the lifecycle tests.
final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    func increment() {
        lock.lock()
        count += 1
        lock.unlock()
    }

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }
}

final class Box<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Value

    init(_ value: Value) {
        stored = value
    }

    var value: Value {
        get {
            lock.lock()
            defer { lock.unlock() }
            return stored
        }
        set {
            lock.lock()
            stored = newValue
            lock.unlock()
        }
    }
}
