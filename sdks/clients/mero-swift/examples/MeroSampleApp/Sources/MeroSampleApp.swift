import MeroKit
import MeroKitTestSupport
import MeroKitUI
import SwiftUI

/// The MeroKit sample iOS app: the SDK Explorer (Home, Chat, SDK) on a
/// Calimero 1.0 cluster.
///
/// - Normal launch: Calimero Cloud sign-in with a passkey on this device, or
///   a development cluster (`merod local`) by URL with mock attestation.
/// - `-uitest-mock`: the same UI against an in-app cluster (MeroKitTestSupport's
///   `FakeCloud` behind a `FakeCloudNode`, which runs the real sealed channel
///   and mock attestation). This is what the XCUITest suite drives, so it
///   needs no network and stays stable in CI.
@main
struct MeroSampleApp: App {
    @StateObject private var session: MeroSession

    init() {
        if CommandLine.arguments.contains("-uitest-mock") {
            _session = StateObject(wrappedValue: Self.mockSession())
        } else {
            _session = StateObject(wrappedValue: MeroSession.live())
        }
    }

    var body: some Scene {
        WindowGroup {
            ExplorerRootView().environmentObject(session)
        }
    }

    /// An in-app cluster: a development node at the default URL, and a
    /// "cloud" node whose mock quote a production policy refuses.
    @MainActor
    private static func mockSession() -> MeroSession {
        let cloud = FakeCloud()
        let local = Bundle.main.object(forInfoDictionaryKey: "DefaultNodeURL") as? String ?? "http://localhost:2501"
        FakeCloudNode.install([
            FakeCloudNode(baseURL: local) { cloud.handle($0) },
            FakeCloudNode(baseURL: "https://cluster.mock") { cloud.handle($0) },
        ])
        let client = MeroClient(
            config: MeroProviderConfig(packageName: ChatService.packageName), store: MemoryCloudSessionStore(),
            session: MockURLProtocol.makeSession(), passkey: SoftPasskey())
        UserDefaults.standard.removeObject(forKey: "lastNodeURL")
        UserDefaults.standard.set("https://cluster.mock", forKey: "lastClusterURL")
        return MeroSession(
            app: MeroAppSession(client: client, callback: .scheme(MeroSession.callbackScheme, path: "consent")))
    }
}
