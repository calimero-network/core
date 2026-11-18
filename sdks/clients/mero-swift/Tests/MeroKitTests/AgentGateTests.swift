import CryptoKit
import Foundation
import MeroKitTestSupport
import XCTest

@testable import MeroKit

/// Port of mero-js `agent-gate/gate.test.ts`, `policy.test.ts` and the AEAD
/// vector of `chacha20poly1305.test.ts` (here CryptoKit's ChaChaPoly).
final class AgentGateTests: XCTestCase {
    private let gateURL = "http://gate.test:8090"

    override func tearDown() {
        MockURLProtocol.reset()
        super.tearDown()
    }

    private func fill(_ byte: UInt8) -> Data { Data(repeating: byte, count: 32) }

    // MARK: - The report-data binding

    // The vectors mero-agent-gate pins (protocol.rs, the_binding_is_pinned).
    func testTheBindingIsTheGatesByteForByte() {
        let claimed = agentKeyBinding(
            provisioningPublic: fill(1), agentId: "ana", signingPublic: fill(2), ownerPublic: fill(4))
        XCTAssertEqual(Hex.encode(claimed), "61c86ed9782e4b7f3d2a0d2146f66934ce5a2c99a79be440d8419c301dfeb1f5")
        XCTAssertEqual(
            Hex.encode(agentKeyBinding(provisioningPublic: fill(1), agentId: "")),
            "315a41492f5b7820a5dd9d72c5223a4a5efd7d4c96f9991d23c9c8b54fb18f6f")
    }

    // MARK: - HPKE with the gate

    func testOpensABundleMeroAgentGateSealed() throws {
        let fixture = try MeroJSON.decode(JSONValue.self, from: try TeeFixture.data("rust-sealed-bundle.json"))
        func bytes(_ key: String) throws -> Data {
            try XCTUnwrap(Data(base64Encoded: fixture[key]?.stringValue ?? ""))
        }
        let info = provisionInfo(
            recipientPublic: try bytes("recipientPublic"), agentId: fixture["agentId"]?.stringValue ?? "")
        let plaintext = try hpkeOpenAuth(
            recipientSecret: try bytes("recipientSecret"), senderPublic: try bytes("senderPublic"), info: info,
            sealed: HpkeSealed(enc: try bytes("enc"), ciphertext: try bytes("ciphertext")))
        let opened = try MeroJSON.decode(JSONValue.self, from: plaintext)
        XCTAssertEqual(opened, ["secrets": ["NEAR_AI_API_KEY": "sk-rust"]])
    }

    func testSealsWhatTheGateOpens() throws {
        let recipient = try X25519.publicKey(secret: fill(0x11))
        let message = Data(#"{"secrets":{"NEAR_AI_API_KEY":"sk-swift"}}"#.utf8)
        let info = provisionInfo(recipientPublic: recipient, agentId: "ana")
        let sealed = try hpkeSealAuth(
            recipientPublic: recipient, senderSecret: fill(0x22), info: info, plaintext: message)
        let sender = try X25519.publicKey(secret: fill(0x22))
        let opened = try hpkeOpenAuth(recipientSecret: fill(0x11), senderPublic: sender, info: info, sealed: sealed)
        XCTAssertEqual(opened, message)
        // Under any other sender key, it does not open: Auth mode names the sender.
        assertTeeRefusal(
            teeThrownMessage {
                _ = try hpkeOpenAuth(
                    recipientSecret: self.fill(0x11), senderPublic: try X25519.publicKey(secret: self.fill(0x33)),
                    info: info, sealed: sealed)
            }, "did not authenticate")
    }

    func testChaChaPolyMatchesTheRFC8439AEADVector() throws {
        let key = SymmetricKey(
            data: try Hex.decodeUnsized("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f", label: "k"))
        let nonce = try ChaChaPoly.Nonce(data: try Hex.decodeUnsized("070000004041424344454647", label: "n"))
        let aad = try Hex.decodeUnsized("50515253c0c1c2c3c4c5c6c7", label: "a")
        let plaintext = Data(
            ("Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, "
                + "sunscreen would be it.").utf8)
        let box = try ChaChaPoly.seal(plaintext, using: key, nonce: nonce, authenticating: aad)
        XCTAssertEqual(Hex.encode(box.tag), "1ae10b594f09e26a7e902ecbd0600691")
        XCTAssertEqual(try ChaChaPoly.open(box, using: key, authenticating: aad), plaintext)
    }

    // MARK: - A gate client

    /// Accepts an echo quote exactly when it is the nonce followed by the binding.
    private let echoVerifier = AnyTransportQuoteVerifier { args in
        guard Hex.encode(Data(base64Encoded: args.quoteB64) ?? Data()) == args.nonce + args.reportDataSuffix else {
            throw TeeTrustError("The quote does not commit to this nonce and binding")
        }
        return true
    }

    private func attest(_ gate: AgentTestGate, _ agentId: String?) async throws -> VerifiedAgent {
        try await attestAgent(
            AttestAgentOptions(
                gateURL: gateURL, agentId: agentId, verifier: echoVerifier, session: MockURLProtocol.makeSession()))
    }

    private func provision(
        _ agent: VerifiedAgent, _ owner: Data, _ secrets: [String: String]
    ) async throws -> ProvisionedAgent {
        try await provisionAgent(
            ProvisionAgentOptions(
                gateURL: gateURL, agent: agent, ownerSecret: owner, secrets: secrets, verifier: echoVerifier,
                session: MockURLProtocol.makeSession()))
    }

    func testClaimsAnUnclaimedAgentAndAFreshQuoteNamesItsOwner() async throws {
        let gate = AgentTestGate()
        gate.install()
        let owner = try generateOwnerKey()
        let agent = try await attest(gate, "a-1")
        let unclaimed = VerifiedAgent(agentId: "a-1", provisioningPublicKey: gate.publicKey.base64EncodedString())
        XCTAssertEqual(agent, unclaimed)
        let done = try await provision(agent, owner.secret, ["NEAR_AI_API_KEY": "sk-1", "MERO_AGENTS_JSON": "{}"])
        XCTAssertTrue(done.claimed)
        XCTAssertEqual(done.written, ["MERO_AGENTS_JSON", "NEAR_AI_API_KEY"])
        XCTAssertEqual(done.ownerPublicKey, owner.publicKey)
        XCTAssertEqual(gate.secrets("a-1")["NEAR_AI_API_KEY"], "sk-1")
    }

    func testRefusesBeforeSendingAnythingAnAgentAnotherKeyClaimed() async throws {
        let gate = AgentTestGate()
        gate.install()
        let first = try generateOwnerKey()
        let agent = try await attest(gate, "a-1")
        _ = try await provision(agent, first.secret, ["A": "1"])

        let second = try generateOwnerKey()
        let claimed = try await attest(gate, "a-1")
        let notYours = await teeAsyncThrownMessage {
            _ = try await self.provision(claimed, second.secret, ["A": "evil"])
        }
        assertTeeRefusal(notYours, "not yours")
        // Trusting a stale quote instead, the gate refuses it.
        do {
            _ = try await provision(agent, second.secret, ["A": "evil"])
            XCTFail("the gate must refuse another key")
        } catch let error as AgentGateError {
            XCTAssertEqual(error.status, 403)
            XCTAssertEqual(error.code, "not_the_owner")
        }
        XCTAssertEqual(gate.secrets("a-1")["A"], "1")
    }

    func testRefusesAQuoteForAnotherAgentThanTheOneAskedAbout() async {
        let gate = AgentTestGate()
        gate.answerFor = "a-2"
        gate.install()
        let refusal = await teeAsyncThrownMessage { _ = try await self.attest(gate, "a-1") }
        assertTeeRefusal(refusal, "the gate answered for \"a-2\"")
    }

    func testCatchesKeysTheQuoteDoesNotCommitTo() async {
        let gate = AgentTestGate()
        gate.swapProvisioningKey = true
        gate.install()
        let refusal = await teeAsyncThrownMessage { _ = try await self.attest(gate, "a-1") }
        assertTeeRefusal(refusal, "does not commit")
    }

    func testReleasesAnAgentAsItsOwnerAndAFreshQuoteNamesNoOwner() async throws {
        let gate = AgentTestGate()
        gate.install()
        let owner = try generateOwnerKey()
        var agent = try await attest(gate, "a-1")
        _ = try await provision(agent, owner.secret, ["A": "1"])
        agent = try await attest(gate, "a-1")
        try await releaseAgent(
            ReleaseAgentOptions(
                gateURL: gateURL, agent: agent, ownerSecret: owner.secret, verifier: echoVerifier,
                session: MockURLProtocol.makeSession()))
        XCTAssertFalse(gate.isClaimed("a-1"))
    }

    func testChecksAgentIdsAndSecretNamesBeforeAnythingIsSent() async throws {
        let gate = AgentTestGate()
        gate.install()
        let badId = await teeAsyncThrownMessage { _ = try await self.attest(gate, "A_1") }
        assertTeeRefusal(badId, "must match")
        let owner = try generateOwnerKey()
        let agent = try await attest(gate, "a-1")
        let badName = await teeAsyncThrownMessage {
            _ = try await self.provision(agent, owner.secret, ["../x": "1"])
        }
        assertTeeRefusal(badName, "must match")
        let none = await teeAsyncThrownMessage { _ = try await self.provision(agent, owner.secret, [:]) }
        assertTeeRefusal(none, "no secrets")
        XCTAssertFalse(gate.isClaimed("a-1"))
        XCTAssertEqual(try ownerPublicKey(owner.secret), owner.publicKey)
        XCTAssertThrowsError(try ownerPublicKey(Data(count: 31)))
    }

    // MARK: - Policy

    private func policy() throws -> AgentPolicy {
        AgentPolicy(
            schemaVersion: 1, role: "agent", profile: "locked-read-only", allowedTcbStatuses: ["uptodate"],
            allowedMrtd: ["0x\(TeeFixture.mrtd.uppercased())"], allowedRtmr0: [TeeFixture.rtmr0],
            allowedRtmr1: [TeeFixture.rtmr1], allowedRtmr2: [TeeFixture.rtmr2], allowedRtmr3: [TeeFixture.rtmr3])
    }

    private func verifier(_ policy: AgentPolicy) throws -> QuoteVerifier {
        let collateral = try TeeFixture.collateral
        return try agentQuoteVerifier(policy, fetchCollateral: { _ in collateral }, now: { TeeFixture.at })
    }

    func testAcceptsAGenuineQuoteOfAnImageThePolicyListsFetchingItsCollateral() async throws {
        let accepted = try await verifier(try policy()).verify(try TeeFixture.args())
        XCTAssertTrue(accepted)
    }

    func testRefusesARegisterOrATCBStatusOutsideThePolicy() async throws {
        var outside = try policy()
        outside.allowedRtmr3 = [String(repeating: "a", count: 96)]
        let args = try TeeFixture.args()
        let rtmr = try verifier(outside)
        assertTeeRefusal(await teeAsyncThrownMessage { _ = try await rtmr.verify(args) }, "RTMR3")
        var status = try policy()
        status.allowedTcbStatuses = ["OutOfDate"]
        let tcb = try verifier(status)
        assertTeeRefusal(await teeAsyncThrownMessage { _ = try await tcb.verify(args) }, "TCB status")
    }

    func testAcceptsNothingFromAnEmptyListOrAStatusNoDCAPVerifierReports() throws {
        var empty = try policy()
        empty.allowedRtmr0 = []
        assertTeeRefusal(teeThrownMessage { _ = try self.verifier(empty) }, "allowed_rtmr0 is empty")
        var unknown = try policy()
        unknown.allowedTcbStatuses = ["Fine"]
        assertTeeRefusal(teeThrownMessage { _ = try self.verifier(unknown) }, "unknown TCB status")
    }

    func testASignedAgentPolicyIsOneTheKMSWorkflowSignedAndAnAgentAllowlist() throws {
        let kms = try TeeFixture.text("mero-kms-v2.3.87.kms-attestation-policy.json")
        let kmsBundle = try TeeFixture.text("mero-kms-v2.3.87.kms-attestation-policy.json.bundle.json")
        // Genuinely signed by the KMS workflow, but the KMS's own policy, not an agent one.
        assertTeeRefusal(
            teeThrownMessage { _ = try verifySignedAgentPolicy(kms, bundle: kmsBundle) }, "not an agent one")
        let node = try TeeFixture.release("2.3.87")
        assertTeeRefusal(
            teeThrownMessage { _ = try verifySignedAgentPolicy(node.body, bundle: node.bundle) }, "release-kms.yaml")
        XCTAssertNotNil(teeThrownMessage { _ = try verifySignedAgentPolicy(kms + " ", bundle: kmsBundle) })
    }
}

/// mero-agent-gate in memory, as its HTTP tests run it: the quote is the
/// report data itself (an echo quoter), so the verifier checks the binding
/// the client computed against the one the gate committed to.
final class AgentTestGate: @unchecked Sendable {
    private struct AgentTestClaim {
        let owner: Data
        let signing: Data
        var secrets: [String: String]
    }

    private let lock = NSLock()
    let provisioningSecret = randomBytes(32)
    private var claims: [String: AgentTestClaim] = [:]
    /// Answer for this agent id instead of the one asked about.
    var answerFor: String?
    /// Name a provisioning key the quote does not commit to.
    var swapProvisioningKey = false

    var publicKey: Data { (try? X25519.publicKey(secret: provisioningSecret)) ?? Data() }

    func secrets(_ agentId: String) -> [String: String] {
        lock.lock()
        defer { lock.unlock() }
        return claims[agentId]?.secrets ?? [:]
    }

    func isClaimed(_ agentId: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return claims[agentId] != nil
    }

    func install() {
        MockURLProtocol.setHandler { [self] request in
            let body = (try? MeroJSON.decode(JSONValue.self, from: FakeCloudNode.body(request))) ?? .null
            do {
                let answer = try handle(request.url?.path ?? "", body)
                return .init(
                    status: 200, headers: ["Content-Type": "application/json"],
                    body: CanonicalJSON.data(answer))
            } catch let refusal as AgentGateError {
                let json = CanonicalJSON.data(
                    ["error": .string(refusal.code ?? ""), "details": .string(refusal.message)])
                return .init(status: refusal.status ?? 500, headers: ["Content-Type": "application/json"], body: json)
            } catch {
                return .init(status: 500, headers: [:], body: Data())
            }
        }
    }

    private func key(_ value: JSONValue?) -> Data { Data(base64Encoded: value?.stringValue ?? "") ?? Data() }

    private func handle(_ path: String, _ body: JSONValue) throws -> JSONValue {
        lock.lock()
        defer { lock.unlock() }
        let provisioning = publicKey
        if path == "/attest" {
            let agentId = answerFor ?? body["agentId"]?.stringValue ?? ""
            let claim = claims[agentId]
            let binding = agentKeyBinding(
                provisioningPublic: provisioning, agentId: agentId, signingPublic: claim?.signing,
                ownerPublic: claim?.owner)
            let quote = key(body["nonceB64"]) + binding
            var out: [String: JSONValue] = [
                "quoteB64": .string(quote.base64EncodedString()),
                "provisioningPublicKeyB64": .string(
                    (swapProvisioningKey ? Data(repeating: 9, count: 32) : provisioning).base64EncodedString()),
            ]
            if !agentId.isEmpty { out["agentId"] = .string(agentId) }
            if let claim {
                out["signingPublicKeyB64"] = .string(claim.signing.base64EncodedString())
                out["ownerPublicKeyB64"] = .string(claim.owner.base64EncodedString())
            }
            return .object(out)
        }
        let agentId = body["agentId"]?.stringValue ?? ""
        let sender = key(body["senderPublicKeyB64"])
        let claim = claims[agentId]
        if let claim, claim.owner != sender {
            throw AgentGateError("not_the_owner", status: 403, code: "not_the_owner")
        }
        let message: JSONValue
        do {
            let plaintext = try hpkeOpenAuth(
                recipientSecret: provisioningSecret, senderPublic: sender,
                info: provisionInfo(recipientPublic: provisioning, agentId: agentId),
                sealed: HpkeSealed(enc: key(body["encappedKeyB64"]), ciphertext: key(body["ciphertextB64"])))
            message = try MeroJSON.decode(JSONValue.self, from: plaintext)
        } catch {
            throw AgentGateError("not_from_sender", status: 403, code: "not_from_sender")
        }
        if path == "/release" {
            guard claim != nil else { throw AgentGateError("unknown_agent", status: 404, code: "unknown_agent") }
            claims[agentId] = nil
            return ["released": .string(agentId)]
        }
        var now = claim ?? AgentTestClaim(owner: sender, signing: randomBytes(32), secrets: [:])
        let secrets = message["secrets"]?.objectValue ?? [:]
        for (name, value) in secrets { now.secrets[name] = value.stringValue }
        claims[agentId] = now
        return ["written": .array(secrets.keys.sorted().map { .string($0) }), "claimed": .bool(claim == nil)]
    }
}
