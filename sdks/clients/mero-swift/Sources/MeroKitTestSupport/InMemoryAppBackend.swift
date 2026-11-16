import Foundation
import MeroKit

// In-memory stand-ins for an app's delegated session, so an app's view models
// are unit-testable with no relay, no cloud and no wallet. (== Kotlin
// mero-testkit `InMemoryAppBackend` / `RecordingContextContract`.)

/// One contract call a ``RecordingContextContract`` saw.
public struct RecordedContractCall: Equatable, Sendable {
    public enum Path: String, Sendable {
        case query
        case execute
    }

    public let path: Path
    public let contextId: String
    public let method: String
    /// The arguments as a ``JSONValue``: an integer beyond 2^53 has lost
    /// digits here; ``argsJSON`` has them all.
    public let args: JSONValue
    /// The arguments exactly as sent, for a call made with JSON bytes
    /// (``ContextContract/query(contextId:method:argsJSON:)``, as a
    /// ``BoundContextContract`` makes it); `nil` for a ``JSONValue`` call.
    public let argsJSON: Data?

    public init(path: Path, contextId: String, method: String, args: JSONValue, argsJSON: Data? = nil) {
        self.path = path
        self.contextId = contextId
        self.method = method
        self.args = args
        self.argsJSON = argsJSON
    }
}

/// A ``ContextContract`` that records every call and answers from a handler
/// (`null` by default). Script the app's contract in the handler, by method.
///
/// A call with JSON bytes (what a ``BoundContextContract`` makes) is recorded
/// with its exact bytes (``RecordedContractCall/argsJSON``) and answered by
/// the raw handler when one is set (``setRawHandler(_:)``), byte for byte;
/// else by the handler, through ``JSONValue``.
public final class RecordingContextContract: ContextContract, @unchecked Sendable {
    public typealias Handler = @Sendable (RecordedContractCall) throws -> JSONValue?
    /// Answers a byte-level call with the return value's JSON, or `nil` to
    /// leave it to the ``Handler``.
    public typealias RawHandler = @Sendable (RecordedContractCall) throws -> Data?

    private let lock = NSLock()
    private var recorded: [RecordedContractCall] = []
    private var handler: Handler
    private var rawHandler: RawHandler?

    public init(handler: @escaping Handler = { _ in .null }) {
        self.handler = handler
    }

    /// Every call so far, in order.
    public var calls: [RecordedContractCall] {
        lock.lock()
        defer { lock.unlock() }
        return recorded
    }

    /// The calls to `method`.
    public func calls(_ method: String) -> [RecordedContractCall] {
        calls.filter { $0.method == method }
    }

    public func setHandler(_ handler: @escaping Handler) {
        lock.lock()
        defer { lock.unlock() }
        self.handler = handler
    }

    /// Answer byte-level calls with exact JSON (an integer beyond 2^53 kept).
    public func setRawHandler(_ handler: RawHandler?) {
        lock.lock()
        defer { lock.unlock() }
        rawHandler = handler
    }

    public func reset() {
        lock.lock()
        defer { lock.unlock() }
        recorded.removeAll()
    }

    public func query(contextId: String, method: String, args: JSONValue) async throws -> JSONValue? {
        try record(RecordedContractCall(path: .query, contextId: contextId, method: method, args: args))
    }

    public func execute(contextId: String, method: String, args: JSONValue) async throws -> JSONValue? {
        try record(RecordedContractCall(path: .execute, contextId: contextId, method: method, args: args))
    }

    public func query(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        try recordRaw(.query, contextId, method, argsJSON)
    }

    public func execute(contextId: String, method: String, argsJSON: Data) async throws -> Data {
        try recordRaw(.execute, contextId, method, argsJSON)
    }

    private func recordRaw(
        _ path: RecordedContractCall.Path, _ contextId: String, _ method: String, _ argsJSON: Data
    ) throws -> Data {
        let args = (try? JSONDecoder().decode(JSONValue.self, from: argsJSON)) ?? .null
        let call = RecordedContractCall(
            path: path, contextId: contextId, method: method, args: args, argsJSON: argsJSON)
        lock.lock()
        recorded.append(call)
        let raw = rawHandler
        let current = handler
        lock.unlock()
        if let answer = try raw?(call) { return answer }
        return CanonicalJSON.data(try current(call) ?? .null)
    }

    private func record(_ call: RecordedContractCall) throws -> JSONValue? {
        lock.lock()
        recorded.append(call)
        let current = handler
        lock.unlock()
        return try current(call)
    }
}

/// A ``MeroAppBackend`` that keeps its namespaces, subgroups, contexts and
/// memberships in memory, and whose contract is a ``RecordingContextContract``.
///
/// It behaves like the relay as far as a view model can tell: founding adds a
/// namespace, creating a subgroup or a context adds it (the creator is a
/// member of a context it creates), joining a context makes the account a
/// member, and redeeming an invitation adds the namespace and gives a
/// relayless account its relay. Names, members and blobs behave the same way:
/// the founder or creator of a group is its `Admin`, a joiner a `Member`;
/// leaving or deleting takes the contexts with it; a blob reads back only
/// through the context it was stored in. Seed state with the `namespaces`,
/// `subgroups`, `members`, `contextNames` and `groupMembers` properties; make every admin-side call fail with ``failure``;
/// push live events with ``emit(_:)``. ``log`` lists what was asked, in order.
public final class InMemoryAppBackend: MeroAppBackend, @unchecked Sendable {
    public let account: String
    public let recording: RecordingContextContract

    private let lock = NSLock()
    private var state = State()

    private struct State {
        var hasRelay = true
        var canSubscribe = true
        var applicationId = String(repeating: "ap", count: 32)
        var failure: Error?
        var haEnabled = true
        var joinRelayURL = "https://relay.test"
        var namespaces: [AppNamespace] = []
        var subgroups: [String: [AppSubgroup]] = [:]
        var rootContexts: [String: [String]] = [:]
        var members: Set<String> = []
        var publicSubgroups: Set<String> = []
        var initArgs: [String: JSONValue] = [:]
        var contextNames: [String: String] = [:]
        var groupData: [String: [String: String]] = [:]
        var groupMembers: [String: [AppMember]] = [:]
        var blobs: [String: (contextId: String, data: Data)] = [:]
        var log: [String] = []
        var next = 0
        var streams:
            [UUID: (contextId: String, continuation: AsyncThrowingStream<AppContextEvent, Error>.Continuation)] =
                [:]
    }

    public init(
        account: String = String(repeating: "ab", count: 32),
        contract: RecordingContextContract = RecordingContextContract()
    ) {
        self.account = account
        self.recording = contract
    }

    // MARK: Scripted state

    /// The contract: ``recording``, or ``SignedOutContextContract`` while ``hasRelay`` is false.
    public var contract: any ContextContract { hasRelay ? recording : SignedOutContextContract() }
    public var hasRelay: Bool {
        get { read { $0.hasRelay } }
        set { write { $0.hasRelay = newValue } }
    }
    public var canSubscribe: Bool {
        get { read { $0.canSubscribe } }
        set { write { $0.canSubscribe = newValue } }
    }
    /// The application a founded or joined namespace runs.
    public var applicationId: String {
        get { read { $0.applicationId } }
        set { write { $0.applicationId = newValue } }
    }
    /// Thrown by every call but ``events(contextId:)`` and the contract while set.
    public var failure: Error? {
        get { read { $0.failure } }
        set { write { $0.failure = newValue } }
    }
    /// What ``foundNamespace(name:)`` reports about hosting.
    public var haEnabled: Bool {
        get { read { $0.haEnabled } }
        set { write { $0.haEnabled = newValue } }
    }
    /// The relay ``joinNamespace(namespaceId:invitation:)`` reports.
    public var joinRelayURL: String {
        get { read { $0.joinRelayURL } }
        set { write { $0.joinRelayURL = newValue } }
    }
    public var namespaces: [AppNamespace] {
        get { read { $0.namespaces } }
        set { write { $0.namespaces = newValue } }
    }
    /// Subgroups by namespace id.
    public var subgroups: [String: [AppSubgroup]] {
        get { read { $0.subgroups } }
        set { write { $0.subgroups = newValue } }
    }
    /// Contexts directly in a group that is not a listed subgroup (a namespace's root), by group id.
    public var rootContexts: [String: [String]] {
        get { read { $0.rootContexts } }
        set { write { $0.rootContexts = newValue } }
    }
    /// The contexts the account is a member of.
    public var members: Set<String> {
        get { read { $0.members } }
        set { write { $0.members = newValue } }
    }
    /// The subgroups created open.
    public var publicSubgroups: Set<String> { read { $0.publicSubgroups } }
    /// The `initArgs` each created context was given, by context id.
    public var initArgs: [String: JSONValue] { read { $0.initArgs } }
    /// Context names (context metadata), by context id.
    public var contextNames: [String: String] {
        get { read { $0.contextNames } }
        set { write { $0.contextNames = newValue } }
    }
    /// The members of each namespace and subgroup, by group id.
    public var groupMembers: [String: [AppMember]] {
        get { read { $0.groupMembers } }
        set { write { $0.groupMembers = newValue } }
    }
    /// The stored blobs' bytes, by blob id.
    public var blobs: [String: Data] { read { $0.blobs.mapValues(\.data) } }
    /// What was asked, in order: `"foundNamespace Team"`, `"joinContext <id>"`, ...
    public var log: [String] { read { $0.log } }

    // MARK: MeroAppBackend

    public func listNamespaces() async throws -> [AppNamespace] {
        try step("listNamespaces") { $0.namespaces }
    }

    public func listSubgroups(namespaceId: String) async throws -> [AppSubgroup] {
        try step("listSubgroups \(namespaceId)") { $0.subgroups[namespaceId] ?? [] }
    }

    public func listContexts(groupId: String) async throws -> [String] {
        try step("listContexts \(groupId)") { Self.contexts(in: groupId, $0) }
    }

    public func isMember(contextId: String) async throws -> Bool {
        try step("isMember \(contextId)") { $0.members.contains(contextId) }
    }

    public func joinContext(contextId: String) async throws {
        try step("joinContext \(contextId)") { _ = $0.members.insert(contextId) }
    }

    public func foundNamespace(name: String) async throws -> AppFoundedNamespace {
        try step("foundNamespace \(name)") { state in
            guard state.hasRelay else { throw SignedOutError(method: "foundNamespace") }
            let id = Self.mint("aa", &state.next)
            state.namespaces.append(
                AppNamespace(id: id, name: name, applicationId: state.applicationId, memberCount: 1))
            state.groupMembers[id] = [AppMember(identity: account, role: GovernanceMemberRole.admin.rawValue)]
            return AppFoundedNamespace(
                namespaceId: id, haEnabled: state.haEnabled, haError: state.haEnabled ? nil : "not hosted (fake)")
        }
    }

    public func createSubgroup(namespaceId: String, name: String, isPublic: Bool) async throws -> String {
        try step("createSubgroup \(namespaceId) \(name) \(isPublic ? "open" : "restricted")") { state in
            let id = Self.mint("bb", &state.next)
            state.subgroups[namespaceId, default: []].append(AppSubgroup(groupId: id, name: name, contextIds: []))
            if isPublic { state.publicSubgroups.insert(id) }
            state.groupMembers[id] = [AppMember(identity: account, role: GovernanceMemberRole.admin.rawValue)]
            return id
        }
    }

    /// A service's instance (``MeroAppBackend/createServiceContext(groupId:applicationId:service:name:initArgs:)``)
    /// is logged with `applicationId` `package#service`.
    public func createContext(
        groupId: String, applicationId: String, name: String, initArgs: JSONValue
    ) async throws -> String {
        try step("createContext \(groupId) \(applicationId) \(name)") { state in
            let id = Self.mint("cc", &state.next)
            var placed = false
            for (namespace, list) in state.subgroups {
                guard let index = list.firstIndex(where: { $0.groupId == groupId }) else { continue }
                let old = list[index]
                state.subgroups[namespace]?[index] = AppSubgroup(
                    groupId: old.groupId, name: old.name, contextIds: old.contextIds + [id])
                placed = true
            }
            if !placed { state.rootContexts[groupId, default: []].append(id) }
            state.members.insert(id)
            state.initArgs[id] = initArgs
            state.contextNames[id] = name
            return id
        }
    }

    public func createInvitation(namespaceId: String) async throws -> OrgInvitation {
        try step("createInvitation \(namespaceId)") { _ in Self.invitation(namespaceId: namespaceId) }
    }

    public func joinNamespace(
        namespaceId: String, invitation: OrgInvitation
    ) async throws -> AppJoinedNamespace {
        try step("joinNamespace \(namespaceId)") { state in
            if !state.namespaces.contains(where: { $0.id == namespaceId }) {
                state.namespaces.append(AppNamespace(id: namespaceId, name: nil, applicationId: state.applicationId))
            }
            if !(state.groupMembers[namespaceId] ?? []).contains(where: { $0.identity == account }) {
                state.groupMembers[namespaceId, default: []].append(
                    AppMember(identity: account, role: GovernanceMemberRole.member.rawValue))
            }
            state.hasRelay = true
            return AppJoinedNamespace(namespaceId: namespaceId, published: true, nodeURL: state.joinRelayURL)
        }
    }

    // MARK: Names

    public func listContextEntries(groupId: String) async throws -> [AppContextEntry] {
        try step("listContextEntries \(groupId)") { state in
            Self.contexts(in: groupId, state).map { AppContextEntry(id: $0, name: state.contextNames[$0]) }
        }
    }

    /// `nil` for a group this backend does not know.
    public func groupMetadata(groupId: String) async throws -> AppMetadata? {
        try step("groupMetadata \(groupId)") { state in
            guard let name = Self.groupName(groupId, state) else { return nil }
            return AppMetadata(name: name, data: state.groupData[groupId] ?? [:])
        }
    }

    public func contextMetadata(groupId: String, contextId: String) async throws -> AppMetadata? {
        try step("contextMetadata \(groupId) \(contextId)") { state in
            state.contextNames[contextId].map { AppMetadata(name: $0) }
        }
    }

    /// Renames the namespace or subgroup in the listings; throws for an unknown group.
    public func setGroupName(groupId: String, name: String) async throws {
        try step("setGroupName \(groupId) \(name)") { state in
            if let index = state.namespaces.firstIndex(where: { $0.id == groupId }) {
                let old = state.namespaces[index]
                state.namespaces[index] = AppNamespace(
                    id: old.id, name: name, applicationId: old.applicationId, memberCount: old.memberCount)
                return
            }
            for (namespace, list) in state.subgroups {
                guard let index = list.firstIndex(where: { $0.groupId == groupId }) else { continue }
                state.subgroups[namespace]?[index] = AppSubgroup(
                    groupId: groupId, name: name, contextIds: list[index].contextIds)
                return
            }
            throw AccountError.invalidInput("no group \(groupId)")
        }
    }

    public func setContextName(groupId: String, contextId: String, name: String) async throws {
        try step("setContextName \(groupId) \(contextId) \(name)") { $0.contextNames[contextId] = name }
    }

    // MARK: Blobs

    public func uploadBlob(contextId: String, data: Data) async throws -> AppBlob {
        try step("uploadBlob \(contextId) \(data.count)") { state in
            guard state.hasRelay else { throw SignedOutError(method: "uploadBlob") }
            let id = Self.mint("dd", &state.next)
            state.blobs[id] = (contextId, data)
            return AppBlob(blobId: id, size: data.count)
        }
    }

    /// Throws unless the blob was stored through `contextId`, as the relay refuses it.
    public func getBlob(blobId: String, contextId: String) async throws -> Data {
        try step("getBlob \(blobId) \(contextId)") { state in
            guard let blob = state.blobs[blobId], blob.contextId == contextId else {
                throw AccountError.invalidInput("no blob \(blobId) in context \(contextId)")
            }
            return blob.data
        }
    }

    // MARK: Leaving and deleting

    /// Drops the namespace, its subgroups and the account's contexts in them.
    public func leaveNamespace(namespaceId: String) async throws {
        try step("leaveNamespace \(namespaceId)") { state in
            state.namespaces.removeAll { $0.id == namespaceId }
            for subgroup in state.subgroups[namespaceId] ?? [] {
                state.members.subtract(subgroup.contextIds)
                Self.dropMember(account, from: subgroup.groupId, &state)
            }
            state.members.subtract(state.rootContexts[namespaceId] ?? [])
            state.subgroups[namespaceId] = nil
            Self.dropMember(account, from: namespaceId, &state)
        }
    }

    /// The subgroup stays listed (it may be open); the account is no longer
    /// in it or its contexts.
    public func leaveSubgroup(groupId: String) async throws {
        try step("leaveSubgroup \(groupId)") { state in
            state.members.subtract(Self.contexts(in: groupId, state))
            Self.dropMember(account, from: groupId, &state)
        }
    }

    public func deleteSubgroup(groupId: String) async throws {
        try step("deleteSubgroup \(groupId)") { state in
            state.members.subtract(Self.contexts(in: groupId, state))
            for namespace in state.subgroups.keys {
                state.subgroups[namespace]?.removeAll { $0.groupId == groupId }
            }
            state.groupMembers[groupId] = nil
            state.publicSubgroups.remove(groupId)
        }
    }

    public func removeContext(groupId: String, contextId: String) async throws {
        try step("removeContext \(groupId) \(contextId)") { state in
            for (namespace, list) in state.subgroups {
                guard let index = list.firstIndex(where: { $0.groupId == groupId }) else { continue }
                let old = list[index]
                state.subgroups[namespace]?[index] = AppSubgroup(
                    groupId: groupId, name: old.name, contextIds: old.contextIds.filter { $0 != contextId })
            }
            state.rootContexts[groupId]?.removeAll { $0 == contextId }
            state.members.remove(contextId)
        }
    }

    // MARK: Members

    public func listMembers(groupId: String) async throws -> [AppMember] {
        try step("listMembers \(groupId)") { $0.groupMembers[groupId] ?? [] }
    }

    /// Throws for someone who is not a member.
    public func setMemberRole(groupId: String, identity: String, role: GovernanceMemberRole) async throws {
        try step("setMemberRole \(groupId) \(identity) \(role.rawValue)") { state in
            guard let index = state.groupMembers[groupId]?.firstIndex(where: { $0.identity == identity }),
                let old = state.groupMembers[groupId]?[index]
            else { throw AccountError.invalidInput("\(identity) is not a member of \(groupId)") }
            state.groupMembers[groupId]?[index] = AppMember(identity: identity, role: role.rawValue, name: old.name)
        }
    }

    public func removeMember(groupId: String, identity: String) async throws {
        try step("removeMember \(groupId) \(identity)") { Self.dropMember(identity, from: groupId, &$0) }
    }

    public func setMemberName(groupId: String, name: String) async throws {
        try step("setMemberName \(groupId) \(name)") { state in
            var list = state.groupMembers[groupId] ?? []
            if let index = list.firstIndex(where: { $0.identity == account }) {
                list[index] = AppMember(identity: account, role: list[index].role, name: name)
            } else {
                list.append(AppMember(identity: account, role: GovernanceMemberRole.member.rawValue, name: name))
            }
            state.groupMembers[groupId] = list
        }
    }

    // MARK: Events

    public func events(contextId: String) -> AsyncThrowingStream<AppContextEvent, Error> {
        guard canSubscribe else { return AsyncThrowingStream { $0.finish() } }
        let id = UUID()
        return AsyncThrowingStream { continuation in
            write { $0.streams[id] = (contextId, continuation) }
            continuation.onTermination = { [weak self] _ in self?.write { $0.streams[id] = nil } }
        }
    }

    // MARK: Driving it

    /// Deliver `event` to every open ``events(contextId:)`` stream for its context.
    public func emit(_ event: AppContextEvent) {
        let targets = read { $0.streams.values.filter { $0.contextId == event.contextId }.map(\.continuation) }
        for continuation in targets { continuation.yield(event) }
    }

    /// Finish every open event stream, with `error` when given.
    public func finishEvents(throwing error: Error? = nil) {
        let targets = read { $0.streams.values.map(\.continuation) }
        for continuation in targets { continuation.finish(throwing: error) }
    }

    /// An invitation to `namespaceId` shaped like a real one (`inv_…`).
    public static func invitation(namespaceId: String) -> OrgInvitation {
        OrgInvitation(namespaceId: namespaceId, token: "inv_" + String(namespaceId.prefix(16)))
    }

    // MARK: Plumbing

    private static func mint(_ prefix: String, _ next: inout Int) -> String {
        next += 1
        let digits = String(next, radix: 16)
        return prefix + String(repeating: "0", count: 64 - prefix.count - digits.count) + digits
    }

    private static func contexts(in groupId: String, _ state: State) -> [String] {
        for list in state.subgroups.values {
            if let found = list.first(where: { $0.groupId == groupId }) { return found.contextIds }
        }
        return state.rootContexts[groupId] ?? []
    }

    private static func groupName(_ groupId: String, _ state: State) -> String?? {
        if let namespace = state.namespaces.first(where: { $0.id == groupId }) { return .some(namespace.name) }
        for list in state.subgroups.values {
            if let found = list.first(where: { $0.groupId == groupId }) { return .some(found.name) }
        }
        return nil
    }

    private static func dropMember(_ identity: String, from groupId: String, _ state: inout State) {
        state.groupMembers[groupId]?.removeAll { $0.identity == identity }
    }

    private func step<T>(_ entry: String, _ body: (inout State) throws -> T) throws -> T {
        lock.lock()
        defer { lock.unlock() }
        state.log.append(entry)
        if let failure = state.failure { throw failure }
        return try body(&state)
    }

    private func read<T>(_ body: (State) -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body(state)
    }

    private func write(_ body: (inout State) -> Void) {
        lock.lock()
        defer { lock.unlock() }
        body(&state)
    }
}
