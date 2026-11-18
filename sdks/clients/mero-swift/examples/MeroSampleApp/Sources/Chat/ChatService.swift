import Foundation
import MeroKit
import MeroKitUI

// MARK: - Wire models (mero-chat contract, snake_case)

/// A single message as returned by mero-chat's `get_messages` / `send_message`.
/// Only the fields the UI needs are decoded; the rest are ignored.
///
/// mero-chat no longer puts the sender's name on a message; it is looked up
/// in `get_profiles` by the sender's identity (see ``ChatService/loadMessages(_:)``).
/// A `sender_username`, when an older contract sends one, is still read.
struct ChatMessage: Decodable, Identifiable {
    let id: String
    let text: String
    var senderUsername: String
    let sender: String
    /// Seconds since the epoch.
    let timestamp: Int
    let deleted: Bool?

    enum CodingKeys: String, CodingKey {
        case id, text, sender, timestamp, deleted
        case senderUsername = "sender_username"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        text = try c.decode(String.self, forKey: .text)
        sender = try c.decode(String.self, forKey: .sender)
        timestamp = try c.decode(Int.self, forKey: .timestamp)
        deleted = try c.decodeIfPresent(Bool.self, forKey: .deleted)
        senderUsername = try c.decodeIfPresent(String.self, forKey: .senderUsername) ?? ""
    }
}

/// mero-chat `get_profiles`: a member's identity and chosen name.
struct ChatProfile: Decodable {
    let identity: String
    let username: String
}

struct ChatMessagePage: Decodable {
    let totalCount: Int
    let messages: [ChatMessage]
    let startPosition: Int

    enum CodingKeys: String, CodingKey {
        case messages
        case totalCount = "total_count"
        case startPosition = "start_position"
    }
}

/// mero-chat `get_info` → the channel/DM shape.
struct ChatContextInfo: Decodable {
    let name: String
    let contextType: String
    let description: String

    enum CodingKeys: String, CodingKey {
        case name, description
        case contextType = "context_type"
    }
}

// MARK: - View models

struct ChatSpace: Identifiable, Equatable {
    let id: String  // namespaceId
    let name: String
    /// The namespace's target application (what a new channel runs).
    var applicationId: String = ""
}

struct ChatChannel: Identifiable, Equatable {
    let id: String  // contextId
    let groupId: String
    let contextId: String
    /// The member identity to execute as — node backend only; a relay writes
    /// as the account, through a warrant.
    let executorId: String
    let name: String
    let kind: String
}

/// Where ``ChatService/spaces`` stands. Mirrors the Kotlin sample's `SpacesLoad`.
enum SpacesLoad {
    /// Not listed yet.
    case pending
    /// Listed: `spaces` is the answer, empty or not.
    case loaded
    /// The listing failed, or the relay session it needs could not be had.
    case failed
}

// MARK: - ChatService

/// A native mero-chat frontend on Calimero 1.0: spaces (orgs), channels
/// (teams, each with one mero-chat app instance), messages (views and
/// intents), invite and join (org invite tokens). Everything goes through
/// MeroKit's app layer (`MeroAppBackend`), the same calls the native apps make.
@MainActor
final class ChatService: ObservableObject {
    /// mero-chat (`apps/mero-chat`), as the app registry publishes it.
    nonisolated static let packageName = "com.calimero.chat"

    /// The package channels run. On 1.0 a channel's app instance installs it
    /// from the cluster's registry when the cluster lacks it, so there is no
    /// install step.
    @Published var appId: String? = ChatService.packageName
    @Published var spaces: [ChatSpace] = []
    /// Where the spaces listing stands. Only `.loaded` with no spaces means
    /// "no spaces yet": a listing that failed is not an empty one.
    @Published private(set) var spacesLoad: SpacesLoad = .pending
    @Published var channels: [ChatChannel] = []
    @Published var messages: [ChatMessage] = []
    @Published var status = ""
    /// Whether the last status is a failure (drives its colour).
    @Published var statusIsError = false
    @Published var busy = false
    @Published var username: String

    let backend: any MeroAppBackend
    let account: String
    /// Bumped by every `loadSpaces()`: only the newest one's answer is applied.
    private var spacesGeneration = 0
    private var spacesError: String?

    init(backend: any MeroAppBackend, username: String) {
        self.backend = backend
        self.account = backend.account
        self.username = username.isEmpty ? "dev" : username
    }

    /// Nothing to install on 1.0: list the spaces.
    func setup() async {
        await loadSpaces()
    }

    func detectInstalled() async {}

    /// Live events for a channel's app instance (sealed SSE).
    func eventStream(_ channel: ChatChannel) -> AsyncThrowingStream<AppContextEvent, Error> {
        backend.events(contextId: channel.contextId)
    }

    // MARK: spaces

    func loadSpaces() async {
        spacesGeneration += 1
        let generation = spacesGeneration
        do {
            let all = try await backend.listNamespaces()
            guard generation == spacesGeneration else { return }
            spaces = all.map { ChatSpace(id: $0.id, name: $0.name ?? "Space", applicationId: $0.applicationId) }
            spacesLoad = .loaded
            if let spacesError, status == spacesError { say("") }
            spacesError = nil
        } catch {
            guard generation == spacesGeneration else { return }
            spacesLoad = .failed
            spacesError = "Couldn't load spaces: \(short(error))"
            say(spacesError ?? "", error: true)
        }
    }

    var canCreateSpaces: Bool { true }

    /// Offer "New space" only once the listing answered, so a failed one can't lead to a duplicate.
    var offerNewSpace: Bool { spacesLoad == .loaded }

    func createSpace(_ name: String) async {
        await run("Creating space \(name)…") {
            let founded = try await self.backend.foundNamespace(name: name)
            try? await self.backend.setMemberName(groupId: founded.namespaceId, name: self.username)
            self.say("Space created: \(name)")
            await self.loadSpaces()
        }
    }

    // MARK: channels

    func loadChannels(_ space: ChatSpace) async {
        channels = []
        do {
            var out: [ChatChannel] = []
            for team in try await backend.listSubgroups(namespaceId: space.id) {
                guard let app = team.contextIds.first else { continue }
                out.append(await channel(contextId: app, groupId: team.groupId, fallback: team.name))
            }
            channels = out.filter { $0.kind != "Dm" }
        } catch {
            say("Couldn't load channels: \(short(error))", error: true)
        }
    }

    private func channel(contextId: String, groupId: String, fallback: String?) async -> ChatChannel {
        let info: ChatContextInfo? = try? await read(contextId, "get_info")
        return ChatChannel(
            id: contextId, groupId: groupId, contextId: contextId, executorId: account,
            name: info?.name ?? fallback ?? "channel", kind: info?.contextType ?? "Channel")
    }

    func createChannel(in space: ChatSpace, name: String, open: Bool) async {
        await run("Creating #\(name)…") {
            let team = try await self.backend.createSubgroup(namespaceId: space.id, name: name, isPublic: open)
            let app = try await self.backend.createContext(
                groupId: team, applicationId: Self.packageName, name: name, initArgs: self.initArgs(name: name))
            let _: JSONValue? = try? await self.write(
                app, "set_profile", args: ["username": .string(self.username), "avatar": .null])
            self.say("Channel #\(name) created")
            await self.loadChannels(space)
        }
    }

    // MARK: messages

    func loadMessages(_ channel: ChatChannel) async {
        do {
            let page: ChatMessagePage = try await read(
                channel.contextId, "get_messages",
                args: ["parent_message": .null, "limit": .number(50), "offset": .number(0), "search_term": .null])
            // Names come from the channel's profiles, keyed by sender identity.
            let profiles: [ChatProfile] = (try? await read(channel.contextId, "get_profiles")) ?? []
            let names = Dictionary(profiles.map { ($0.identity, $0.username) }, uniquingKeysWith: { first, _ in first })
            messages = page.messages.filter { $0.deleted != true }.map { message in
                var named = message
                if named.senderUsername.isEmpty, let name = names[message.sender] { named.senderUsername = name }
                return named
            }
        } catch { say("Couldn't load messages: \(short(error))", error: true) }
    }

    func sendMessage(_ channel: ChatChannel, _ text: String) async {
        guard !text.isEmpty else { return }
        // Seconds: mero-chat refuses a timestamp more than 5 minutes ahead of the node's clock.
        let ts = Int(Date().timeIntervalSince1970)
        do {
            let _: JSONValue? = try await write(
                channel.contextId, "send_message",
                args: [
                    "message": .string(text), "mentions": .array([]), "mentions_usernames": .array([]),
                    "parent_message": .null, "timestamp": .number(Double(ts)), "files": .null, "images": .null,
                ])
            await loadMessages(channel)
        } catch { say("Couldn't send: \(short(error))", error: true) }
    }

    // MARK: invite / join

    var canInvite: Bool { true }

    func makeInvite(_ space: ChatSpace) async -> String? {
        do {
            let minted = try await backend.createInvitation(namespaceId: space.id)
            let code = try OrgInvitation(namespaceId: space.id, token: minted.token, name: space.name).encoded()
            say("Invite ready. Copy or share it.")
            return code
        } catch {
            say("Couldn't create an invite: \(short(error))", error: true)
            return nil
        }
    }

    /// Join a space from an invite code or link. Returns whether it joined.
    @discardableResult
    func joinSpace(_ inviteCode: String) async -> Bool {
        busy = true
        defer { busy = false }
        say("Reading invite…")
        guard let invite = OrgInvitation.parse(inviteCode) else {
            say("That isn't a valid invite code.", error: true)
            return false
        }
        do {
            let spaceName = invite.name ?? "the space"
            say("Joining \(spaceName)…")
            let joined = try await backend.joinNamespace(namespaceId: invite.namespaceId, invitation: invite)
            try? await backend.setMemberName(groupId: joined.namespaceId, name: username)
            await loadSpaces()
            if let space = spaces.first(where: { $0.id == joined.namespaceId }) {
                await loadChannels(space)
                for channel in channels {
                    let _: JSONValue? = try? await write(
                        channel.contextId, "set_profile", args: ["username": .string(username), "avatar": .null])
                }
            }
            say("Joined \(spaceName)")
            return true
        } catch {
            say("Couldn't join: \(short(error))", error: true)
            return false
        }
    }

    /// Re-read a space's channels.
    func resync(_ space: ChatSpace) async {
        await run("Refreshing \(space.name)…") {
            await self.loadChannels(space)
            self.say(self.channels.isEmpty ? "No channels yet." : "\(self.channels.count) channel(s)")
        }
    }

    // MARK: helpers

    private func initArgs(name: String) -> JSONValue {
        [
            "name": .string(name), "context_type": "Channel", "description": "",
            "created_at": .number(Double(Int(Date().timeIntervalSince1970))),
            "creator_username": .string(username),
        ]
    }

    /// A read: `POST /apps/{app}/views`.
    private func read<T: Decodable>(_ app: String, _ method: String, args: [String: JSONValue] = [:]) async throws -> T
    {
        let result = try await backend.contract.query(contextId: app, method: method, args: .object(args)) ?? .null
        return try JSONDecoder().decode(T.self, from: try JSONEncoder().encode(result))
    }

    /// A write: `POST /apps/{app}/intents`.
    private func write<T: Decodable>(_ app: String, _ method: String, args: [String: JSONValue] = [:]) async throws -> T
    {
        let result = try await backend.contract.execute(contextId: app, method: method, args: .object(args)) ?? .null
        return try JSONDecoder().decode(T.self, from: try JSONEncoder().encode(result))
    }

    private func say(_ message: String, error: Bool = false) {
        status = message
        statusIsError = error
    }

    private func run(_ message: String, _ body: @escaping () async throws -> Void) async {
        busy = true
        say(message)
        defer { busy = false }
        do { try await body() } catch {
            say("\(message.replacingOccurrences(of: "…", with: "")) failed: \(short(error))", error: true)
        }
    }

    private func short(_ error: Error) -> String {
        if let u = error as? URLError { return "network \(u.code)" }
        // A contract's refusal in its own words.
        if let app = error as? AppError { return app.appCode.isEmpty ? app.message : app.appCode }
        return MeroClient.describe(error)
    }
}
