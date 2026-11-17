import XCTest

final class MemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String: Data] = [:]
    private(set) var saveCount = 0

    func load(account: String) throws -> Data? {
        lock.lock()
        defer { lock.unlock() }
        return items[account]
    }

    func save(_ data: Data, account: String) throws {
        lock.lock()
        items[account] = data
        saveCount += 1
        lock.unlock()
    }

    func delete(account: String) throws {
        lock.lock()
        items.removeValue(forKey: account)
        lock.unlock()
    }
}

final class IdentityVaultTests: XCTestCase {
    private var root: URL!
    private var store: MemorySecretStore!
    private var vault: IdentityVault!

    override func setUpWithError() throws {
        try super.setUpWithError()
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("fv6-identity-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        store = MemorySecretStore()
        vault = IdentityVault(store: store)
    }

    override func tearDownWithError() throws {
        if let root, FileManager.default.fileExists(atPath: root.path) {
            try FileManager.default.removeItem(at: root)
        }
        try super.tearDownWithError()
    }

    private var identityFile: URL {
        IdentityVault.identityURL(storageRoot: root)
    }

    func testRestoreWritesTheStoredIdentity() throws {
        let secret = Data("stored-pem".utf8)
        try store.save(secret, account: IdentityVault.account)

        XCTAssertTrue(try vault.restore(into: root))
        XCTAssertEqual(try Data(contentsOf: identityFile), secret)
    }

    func testRestoreWithoutAStoredIdentityCreatesNothing() throws {
        XCTAssertFalse(try vault.restore(into: root))
        XCTAssertFalse(FileManager.default.fileExists(atPath: identityFile.path))
    }

    func testRestoreNeverOverwritesAnExistingFile() throws {
        let onDisk = Data("on-disk-pem".utf8)
        try onDisk.write(to: identityFile)
        try store.save(Data("keychain-pem".utf8), account: IdentityVault.account)

        XCTAssertFalse(try vault.restore(into: root))
        XCTAssertEqual(try Data(contentsOf: identityFile), onDisk)
    }

    func testCaptureStoresTheIdentityAndRemovesThePlaintext() throws {
        let secret = Data("runtime-pem".utf8)
        try secret.write(to: identityFile)

        XCTAssertTrue(try vault.capture(from: root))
        XCTAssertEqual(try store.load(account: IdentityVault.account), secret)
        XCTAssertFalse(FileManager.default.fileExists(atPath: identityFile.path))
    }

    func testCaptureWithoutAFileIsANoOp() throws {
        XCTAssertFalse(try vault.capture(from: root))
        XCTAssertNil(try store.load(account: IdentityVault.account))
        XCTAssertEqual(store.saveCount, 0)
    }

    func testCaptureThenRestoreKeepsTheSameIdentityBytes() throws {
        let secret = Data("identity-bytes".utf8)
        try secret.write(to: identityFile)

        XCTAssertTrue(try vault.capture(from: root))
        XCTAssertFalse(FileManager.default.fileExists(atPath: identityFile.path))
        XCTAssertTrue(try vault.restore(into: root))
        XCTAssertEqual(try Data(contentsOf: identityFile), secret)
    }

    func testTheIdentityIsNotScopedToANetworkName() throws {
        // The PEM sits at the storage root and the Rust runtime keeps one node
        // identity per storage root, so the account must not follow the network
        // name: rejoining under a different name has to reuse the same key.
        try store.save(Data("root-pem".utf8), account: IdentityVault.account)

        XCTAssertTrue(try vault.restore(into: root))
        XCTAssertEqual(try Data(contentsOf: identityFile), Data("root-pem".utf8))
    }

    func testPerNetworkAccountsAreIgnored() throws {
        // Legacy layout: the identity used to be stored under the network name.
        try store.save(Data("legacy-mesh-pem".utf8), account: "mesh")

        XCTAssertFalse(try vault.restore(into: root))
        XCTAssertFalse(FileManager.default.fileExists(atPath: identityFile.path))
    }
}
