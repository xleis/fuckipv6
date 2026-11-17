import Foundation

/// Reference-counted guard for a raw FFI handle that worker threads use.
///
/// The Rust endpoint is a `Box`: freeing it while a `send`/`recv` call is still
/// borrowing it is a use-after-free. Every call therefore brackets itself with
/// `acquire()` / `release()`, and `close()` only frees once the last user has
/// left. A close that races with an in-flight call is not an error: it is
/// recorded as deferred and completed by whoever decrements the count to zero.
///
/// Invariants:
/// - `free` runs at most once, outside the lock.
/// - `acquire()` never succeeds after `close()`.
/// - Every successful `acquire()` is paired with exactly one `release()`.
final class HandleGuard: @unchecked Sendable {
    enum CloseOutcome: Equatable {
        /// No users were in flight; the handle is freed.
        case freed
        /// Users are still running; the last `release()` frees the handle.
        case deferred(users: Int)
        /// Already closed (and possibly already freed).
        case alreadyClosed
    }

    private let lock = NSCondition()
    private var users = 0
    private var closed = false
    private var freed = false
    private let free: () -> Void

    init(free: @escaping () -> Void) {
        self.free = free
    }

    /// Registers one in-flight use. Returns false once the handle is closed,
    /// in which case the caller must not touch the raw handle.
    func acquire() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !closed else { return false }
        users += 1
        return true
    }

    /// Ends one in-flight use, freeing the handle when it was the last user of
    /// a closed guard.
    func release() {
        var runFree = false
        lock.lock()
        if users > 0 {
            users -= 1
        }
        if closed && users == 0 && !freed {
            freed = true
            runFree = true
        }
        lock.broadcast()
        lock.unlock()
        if runFree {
            free()
        }
    }

    /// Closes the guard. Never blocks and never frees a handle that a user is
    /// still holding.
    @discardableResult
    func close() -> CloseOutcome {
        var runFree = false
        var outcome = CloseOutcome.alreadyClosed
        lock.lock()
        if closed {
            outcome = .alreadyClosed
        } else {
            closed = true
            if users == 0 && !freed {
                freed = true
                runFree = true
                outcome = .freed
            } else {
                outcome = .deferred(users: users)
            }
        }
        lock.broadcast()
        lock.unlock()
        if runFree {
            free()
        }
        return outcome
    }

    /// Waits until the guard is closed and every user has left. Returns false
    /// when the deadline expires first.
    func waitUntilFree(timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        lock.lock()
        defer { lock.unlock() }
        while !(closed && users == 0) {
            if !lock.wait(until: deadline) && !(closed && users == 0) {
                return false
            }
        }
        return true
    }

    var userCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return users
    }

    var isClosed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return closed
    }
}
