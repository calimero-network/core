import Foundation
import MeroAPI
import MeroKit

// A runnable tour of MeroKit against a Calimero 1.0 cluster.
//
//   swift run MeroExample
//
// With no environment it runs the OFFLINE part: a software passkey, the typed
// errors a node's refusals become, and an invitation link round trip.
//
// Point it at a development cluster (`merod local`, mock attestation) to run
// the ONLINE part: sign up with a software passkey, list apps, and call one.
//
//   MERO_NODE_URL=http://127.0.0.1:2501 MERO_APP=<app id> swift run MeroExample

func env(_ key: String) -> String? {
    guard let value = ProcessInfo.processInfo.environment[key], !value.isEmpty else { return nil }
    return value
}

func section(_ title: String) {
    print("\n== \(title) ==")
}

@main
struct MeroExample {
    static func main() async {
        section("Software passkey")
        let passkey = SoftPasskey()
        print("credential id:", passkey.credentialId, "rp:", passkey.rpId, "origin:", passkey.origin)

        section("Typed errors")
        let refusal: JSONValue = [
            "error": "APP_ERROR", "message": "the app refused the call",
            "details": ["index": 3, "error": ["code": "Taken", "fields": ["name": "ada"]]],
        ]
        if let error = errorFrom(status: 422, body: refusal) as? AppError {
            print("AppError appCode:", error.appCode, "fields:", error.fields)
        }

        section("Invitation link payload")
        let invitation = OrgInvitation(namespaceId: "org_example", token: "inv_example")
        if let token = try? InviteCodec.encode(invitation) {
            print("link:", InviteLink.invitation(token: token, slug: "com.calimero.chat"))
        }

        guard let node = env("MERO_NODE_URL") else {
            print("\nSet MERO_NODE_URL (a `merod local` node) to run the online part.")
            return
        }
        section("Online: \(node)")
        let client = CalimeroClient(CalimeroClientOptions(baseUrl: node, policy: .development))
        do {
            let session = try await client.signUp(passkey, name: env("MERO_NAME") ?? "example")
            print("signed up as", session.accountId)
            let apps = try await client.apps.list().apps
            print("apps:", apps.map(\.name))
            if let app = env("MERO_APP") {
                let answer = try await client.view(app, env("MERO_METHOD") ?? "get", [:])
                print("view result:", answer.result ?? .null)
            }
        } catch {
            print("failed:", error)
        }
    }
}
