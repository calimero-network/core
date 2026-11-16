import CryptoKit
import XCTest

@testable import MeroKit

final class AppHostTests: XCTestCase {
    // Vectors from mdma `mdma_common.app_host` (core `app_host::label`).
    func testLabelsMatchCoreAndTheManager() {
        XCTAssertEqual(
            AppHost.label(package: "com.calimero.chat"), "pjfzih7fgm2e33figvvm2hcvlbr3egcxy3jjn6zhuzdlxkz4hfdq")
        XCTAssertEqual(
            AppHost.clusterLabel(cluster: "eu-shared-1", package: "com.calimero.chat"),
            "5eolix2xlsehjkbd2l5262oevb65zvq4dkgo66zlxpfc4os34una")
    }

    func testOriginIsThePackagesHostOrTheClustersOwn() {
        XCTAssertEqual(
            AppHost.origin(package: "com.calimero.chat").absoluteString,
            "https://pjfzih7fgm2e33figvvm2hcvlbr3egcxy3jjn6zhuzdlxkz4hfdq.apps.calimero.network")
        XCTAssertEqual(
            AppHost.origin(package: "com.calimero.chat", cluster: "eu-shared-1").absoluteString,
            "https://5eolix2xlsehjkbd2l5262oevb65zvq4dkgo66zlxpfc4os34una.apps.calimero.network")
    }
}

final class ServedPageHandoffTests: XCTestCase {
    // Sealed by apps `@calimero-apps/passkey/native` (`sealNativeSession`), WebCrypto.
    static let jsPrivateKey = "EpEiMtOxmJcC8Oyk4yxIbQYSzNzstVYDlfteYP99sc0"
    static let jsPublicKey = "BPm9Ej3fpM78ghV7ScdMix1F2coNyd4CzOo4wYgGl6J7HwWpT_RF9F2UFB2hmWMC18FNL7qERY4X4HC1qTzcRto"
    static let jsState = "c3RhdGUtZm9yLXRoaXMtc2lnbi1pbg"
    static let jsBox = [
        "BGXAHK69TLTGc5H45ACET70ViGXy_NxiBMP-cyei7hsPwDzi7MKUiC7fW8VglZEa8PjpOLhchHRKFKk1Yl_qnowMAVY8i-E8",
        "-fQFYNUyYQrs3VcdqyQcQ6vAXLym7vjHXmIC8Rt2Vu4PFi_wyBGthxCsqp8VtVrmiACH-yqSw2ENb6F8WXUhIl9UInB-osdL",
        "nEWIwqQJcVswEAPjbKlEtWCwY1PZVUVGwBr7PwJ5oSwITiQDeYI9caY5LL2yFXPYuLRWr65Q8g5148xjWgTbf6E8M-nf_mi9",
        "vA4Vb06bx-L2vF6W-vpz6q_BO0Gq6vONxvcqmeIeHfQTPylNxuRfpAOp",
    ].joined()

    private func jsHandoff() throws -> ServedPageHandoff {
        let key = try P256.KeyAgreement.PrivateKey(rawRepresentation: fromB64url(Self.jsPrivateKey))
        return ServedPageHandoff(privateKey: key, state: Self.jsState)
    }

    private func answer(state: String = jsState, box: String = jsBox) -> URL {
        URL(string: "mero-chat://signed-in#state=\(state)&box=\(box)")!
    }

    func testOpensWhatTheWebPageSealed() throws {
        let handoff = try jsHandoff()
        XCTAssertEqual(handoff.publicKey, Self.jsPublicKey)
        let got = try handoff.open(answer())
        XCTAssertEqual(
            got,
            ServedPageSession(
                v: 1, accountId: "acc_1", sessionToken: "v2.claims.sig", expiresAt: 1_900_000_000,
                refreshToken: "rt_1", nodeUrl: "https://n1.example", nodes: ["https://n2.example"]))
        XCTAssertEqual(got.session.refreshToken, "rt_1")
    }

    func testRefusesAReplayedAnswer() throws {
        let handoff = try jsHandoff()
        _ = try handoff.open(answer())
        XCTAssertThrowsError(try handoff.open(answer())) {
            XCTAssertEqual($0 as? ServedPageSignInError, ServedPageSignInError("this request was already answered"))
        }
    }

    func testRefusesAnotherState() throws {
        XCTAssertThrowsError(try jsHandoff().open(answer(state: "c29tZS1vdGhlci1zaWduLWluLXN0YXRl")))
    }

    func testAnotherKeyCannotOpenTheBox() {
        let other = ServedPageHandoff(privateKey: P256.KeyAgreement.PrivateKey(), state: Self.jsState)
        XCTAssertThrowsError(try other.open(answer())) {
            XCTAssertEqual($0 as? ServedPageSignInError, ServedPageSignInError("the answer could not be opened"))
        }
    }

    func testRefusesAnAnswerWithoutASession() throws {
        XCTAssertThrowsError(try jsHandoff().open(URL(string: "mero-chat://signed-in")!))
        XCTAssertThrowsError(try jsHandoff().open(answer(box: "AAAA")))
    }

    func testThePageURLCarriesStateAndKeyInTheFragment() {
        let handoff = ServedPageHandoff()
        let url = handoff.url(origin: URL(string: "https://abc.apps.calimero.network")!, version: "1.2.0")
        XCTAssertEqual(url.path, "/1.2.0")
        XCTAssertNil(url.query)
        XCTAssertEqual(url.fragment, "native-sign-in&state=\(handoff.state)&key=\(handoff.publicKey)")
        XCTAssertEqual(try fromB64url(handoff.publicKey).count, 65)
        XCTAssertNotEqual(handoff.state, ServedPageHandoff().state)
    }
}

final class ServedPageApprovalTests: XCTestCase {
    // Sealed by apps `@calimero-apps/passkey/native` (`approveNative`), WebCrypto.
    static let jsAnswer =
        "mero-chat://approved#state=c3RhdGUtZm9yLXRoaXMtc2lnbi1pbg&box=BGqRdlG34snUtVCv1fANYLwMq3Mi2-8jey_OJk-v5mj3A2AEulVm2VZeRGLWGy3elP1Y70Gm-2DtmCPSaYmk2N0fTp6LaB-u9muFZDJ2oh4IDknAWoZShiqFEIEDMh3B_SEL-ROmQUbeAAh-WUxipNxyXKonr9mRsJ1uRRv49K31ThwaIeujjSJGpo6_dGLnGlwyR8MMyFxUKv-tm-loT6ZNSjkfjXD_QhtMJSUAWA5Kmp6D4hZ8vbEjCvb3i_3XY1s3FpXSWcqHKqA"

    private func handoff() throws -> ServedPageHandoff {
        let key = try P256.KeyAgreement.PrivateKey(rawRepresentation: fromB64url(ServedPageHandoffTests.jsPrivateKey))
        return ServedPageHandoff(privateKey: key, state: ServedPageHandoffTests.jsState)
    }

    func testOpensTheAssertionTheWebPageSealed() throws {
        let got = try handoff().openApproval(URL(string: Self.jsAnswer)!)
        XCTAssertEqual(
            got, AssertionJSON(credentialId: "cred_1", authenticatorData: "ad", clientDataJson: "cd", signature: "sig"))
    }

    func testASignInCannotBeReadAsAnApprovalOrTheOtherWay() throws {
        let state = ServedPageHandoffTests.jsState
        let signIn = URL(string: "mero-chat://signed-in#state=\(state)&box=\(ServedPageHandoffTests.jsBox)")!
        XCTAssertThrowsError(try handoff().openApproval(signIn))
        XCTAssertThrowsError(try handoff().open(URL(string: Self.jsAnswer)!))
    }

    func testRefusesAReplayedApproval() throws {
        let h = try handoff()
        _ = try h.openApproval(URL(string: Self.jsAnswer)!)
        XCTAssertThrowsError(try h.openApproval(URL(string: Self.jsAnswer)!))
    }

    func testADeclineIsACancelledPasskey() throws {
        let declined = URL(string: "mero-chat://approved#state=\(ServedPageHandoffTests.jsState)&cancelled=1")!
        XCTAssertThrowsError(try handoff().openApproval(declined)) { XCTAssertTrue($0 is PasskeyCancelled) }
    }

    func testTheApprovalURLCarriesTheChallengeInTheFragment() {
        let h = ServedPageHandoff()
        let url = h.url(
            origin: URL(string: "https://x.apps.calimero.network")!, version: "3.1.72",
            fragment: ServedPageHandoff.approveFragment, extra: [("challenge", "AAAA"), ("rpId", "calimero.network")])
        XCTAssertTrue(url.absoluteString.hasPrefix("https://x.apps.calimero.network/3.1.72/#"))
        XCTAssertEqual(
            url.fragment, "native-approve&state=\(h.state)&key=\(h.publicKey)&challenge=AAAA&rpId=calimero.network")
    }
}

final class ServedConsentReturnTests: XCTestCase {
    func testTheConsentPageReturnsToTheServedPageWithoutAFragment() {
        let to = ServedPageHandoff.consentReturnTo(
            origin: URL(string: "https://x.apps.calimero.network/")!, version: "3.1.72")
        XCTAssertEqual(to, "https://x.apps.calimero.network/3.1.72/?native-consent=1")
        XCTAssertNil(URL(string: to)?.fragment)
    }
}
