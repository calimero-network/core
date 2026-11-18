import Foundation
import MeroKit
import MeroKitUI
import os

/// The explorer's session on a Calimero 1.0 cluster, plus a diagnostics log.
///
/// - **Calimero Cloud**: a cluster URL (from Calimero Cloud's console;
///   `CalimeroClusterURL` in Info.plist prefills it), the node attested
///   against signed mero-tee releases, and a passkey on this device
///   (`PlatformPasskey`).
/// - **Manually enter node URL**: a development cluster (`merod local`) with
///   mock attestation allowed, signed in with a software passkey kept in the
///   Keychain for that node (a device passkey cannot belong to `localhost`).
///
/// Both go through MeroKitUI's `MeroAppSession`, as the native apps do:
/// the chat runs on its `MeroAppBackend`.
@MainActor
final class MeroSession: ObservableObject {
    struct LogLine: Identifiable {
        enum Level {
            case info, ok, warn, err, req

            /// SF Symbol for the level (no glyphs in the UI).
            var symbol: String {
                switch self {
                case .info: return "circle.fill"
                case .ok: return "checkmark.circle"
                case .warn: return "exclamationmark.triangle"
                case .err: return "xmark.circle"
                case .req: return "arrow.right"
                }
            }

            var tag: String {
                switch self {
                case .info: return "info"
                case .ok: return "ok"
                case .warn: return "warn"
                case .err: return "error"
                case .req: return "req"
                }
            }

            var osLogType: OSLogType {
                switch self {
                case .info, .ok, .req: return .info
                case .warn: return .default
                case .err: return .error
                }
            }
        }

        let id = UUID()
        let time: Date
        let level: Level
        let text: String
    }

    static let callbackScheme = "mero-sample"

    let app: MeroAppSession
    var client: MeroClient { app.client }

    @Published private(set) var isAuthenticated = false
    @Published var errorMessage: String?
    /// The cluster URL last used for Cloud sign-in.
    @Published var lastClusterURL: String =
        UserDefaults.standard.string(forKey: "lastClusterURL")
        ?? (Bundle.main.object(forInfoDictionaryKey: "CalimeroClusterURL") as? String ?? "")
    /// The development node URL last used, prefilled on the sign-in screen.
    @Published var lastNodeURL: String =
        UserDefaults.standard.string(forKey: "lastNodeURL")
        ?? (Bundle.main.object(forInfoDictionaryKey: "DefaultNodeURL") as? String ?? "http://localhost:2501")
    /// The name a new account registers under.
    @Published var lastName: String = UserDefaults.standard.string(forKey: "lastName") ?? "dev"
    @Published private(set) var logs: [LogLine] = []
    /// One chat service for the whole session.
    @Published private(set) var chat: ChatService?
    @Published private(set) var isLoading = false

    var account: String? { client.account }
    var nodeURL: String? { client.nodeURL }
    /// Signed in to a development cluster (mock attestation).
    var isDevelopment: Bool { client.allowMock }
    var cloud: CalimeroClient? { client.cloud }

    /// A display name for chat and the header.
    var displayName: String {
        if let name = ProcessInfo.processInfo.environment["E2E_USERNAME"], !name.isEmpty { return name }
        if !lastName.isEmpty { return lastName }
        return account.map { "Account \($0.prefix(6))" } ?? ""
    }

    init(app: MeroAppSession) {
        self.app = app
    }

    /// The real session: the device's passkey, the session in the Keychain.
    static func live() -> MeroSession {
        let client = MeroClient(
            config: MeroProviderConfig(packageName: ChatService.packageName),
            store: KeychainCloudSessionStore(service: "network.calimero.merokit.sample"))
        return MeroSession(app: MeroAppSession(client: client, callback: .scheme(callbackScheme, path: "consent")))
    }

    private static let ts: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm:ss.SSS"
        return f
    }()

    /// Mirrors the diagnostics log to the unified log. Lines name spaces,
    /// contexts and accounts, so the text is private: shown with a debugger
    /// attached, redacted in a device's collected logs.
    private static let logger = Logger(subsystem: "network.calimero.merokit.sample", category: "session")

    func log(_ level: LogLine.Level, _ text: String) {
        logs.append(LogLine(time: Date(), level: level, text: text))
        if logs.count > 300 { logs.removeFirst(logs.count - 300) }
        Self.logger.log(level: level.osLogType, "\(level.tag, privacy: .public) \(text, privacy: .private)")
    }

    func clearLogs() { logs.removeAll() }

    func logText() -> String {
        logs.map { "\(Self.ts.string(from: $0.time)) [\($0.level.tag)] \($0.text)" }.joined(separator: "\n")
    }

    // MARK: - Launch

    /// Restore a previous session, or use the e2e dev-node hook (`E2E_NODE`).
    func start() async {
        let env = ProcessInfo.processInfo.environment
        if let node = env["E2E_NODE"], !node.isEmpty {
            await connectNode(node, name: env["E2E_USERNAME"] ?? lastName)
            return
        }
        isLoading = true
        defer { isLoading = false }
        await app.restore()
        if app.isSignedIn {
            log(.ok, "restored session for \(account?.prefix(12) ?? "")…")
            adopt()
        }
    }

    // MARK: - Calimero Cloud: a passkey on this device

    func signInWithCloud(_ rawURL: String, createAccount: Bool) async {
        errorMessage = nil
        guard let url = validURL(rawURL, example: "https://node.your-cluster.calimero.network") else { return }
        lastClusterURL = url
        UserDefaults.standard.set(url, forKey: "lastClusterURL")
        isLoading = true
        defer { isLoading = false }
        log(.req, "attesting \(url) and signing in with a passkey")
        app.connect(nodeURL: url, allowMock: false)
        if createAccount { await app.signUp(name: lastName) } else { await app.signIn() }
        finish()
    }

    // MARK: - A development cluster (merod local)

    func connectNode(_ rawURL: String, name: String) async {
        errorMessage = nil
        guard let url = validURL(rawURL, example: "http://localhost:2501") else { return }
        lastNodeURL = url
        lastName = name.isEmpty ? "dev" : name
        UserDefaults.standard.set(url, forKey: "lastNodeURL")
        UserDefaults.standard.set(lastName, forKey: "lastName")
        isLoading = true
        defer { isLoading = false }
        log(.req, "node \(url) (mock attestation allowed)")
        app.connect(nodeURL: url, allowMock: true)
        await client.signInOrSignUp(name: lastName)
        app.rebuild()
        finish()
    }

    private func finish() {
        if let error = client.errorMessage {
            errorMessage = error
            log(.err, "sign-in failed: \(error)")
        } else if app.isSignedIn {
            log(.ok, "signed in as \(account?.prefix(12) ?? "")…")
            adopt()
        }
    }

    private func adopt() {
        guard let backend = app.backend else { return }
        chat = ChatService(backend: backend, username: displayName)
        isAuthenticated = true
    }

    private func validURL(_ raw: String, example: String) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed), url.scheme == "http" || url.scheme == "https", url.host != nil else {
            errorMessage = "Enter a full URL, like \(example)."
            return nil
        }
        return trimmed
    }

    /// A link the system handed the app: a chat invitation or a consent page's return.
    func handle(_ url: URL) async {
        if await app.handle(url), let link = app.pendingInvitation, let chat {
            app.dismissPendingInvitation()
            await chat.joinSpace(link)
        }
    }

    // MARK: - Sign out

    func logout() async {
        log(.req, "sign out")
        await app.signOut()
        chat = nil
        isAuthenticated = false
        log(.ok, "signed out")
    }
}
