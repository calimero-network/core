import Foundation
import MeroKit

#if canImport(AuthenticationServices) && (os(iOS) || os(macOS))
import AuthenticationServices
#if os(iOS)
import UIKit
#else
import AppKit
#endif

/// The device's passkeys (Face ID, Touch ID, a passkey synced through iCloud
/// Keychain), through `ASAuthorizationController`. (== mero-js `browserPasskey()`.)
///
/// A passkey belongs to the cluster's relying party (`--rp-id`, the node's
/// sign-in domain). The app must name that domain in its Associated Domains
/// entitlement (`webcredentials:<rp-id>`), and the domain must serve an
/// `apple-app-site-association` listing the app; the node must list the
/// origin `https://<rp-id>` among its `--origins`. A development cluster on
/// `localhost` cannot take a platform passkey: use ``SoftPasskey`` there.
public final class PlatformPasskey: NSObject, PasskeyProvider, @unchecked Sendable {
    /// The relying party for registration when the node's options name none.
    public let defaultRpId: String?

    public init(rpId: String? = nil) {
        self.defaultRpId = rpId
    }

    public func create(options: JSONValue) async throws -> RegistrationJSON {
        guard let challenge = options["challenge"]?.stringValue,
            let rpId = options["rp"]?["id"]?.stringValue ?? defaultRpId,
            let userId = options["user"]?["id"]?.stringValue
        else { throw AccountError.protocolViolation("the node's registration options are incomplete") }
        let name = options["user"]?["name"]?.stringValue ?? "Calimero"
        let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: rpId)
        let request = provider.createCredentialRegistrationRequest(
            challenge: try fromB64url(challenge), name: name, userID: try fromB64url(userId))
        return try await perform(request) { credential in
            guard let registration = credential as? ASAuthorizationPlatformPublicKeyCredentialRegistration,
                let attestation = registration.rawAttestationObject
            else { throw AccountError.protocolViolation("the passkey returned no registration") }
            return RegistrationJSON(
                attestationObject: b64url(attestation), clientDataJson: b64url(registration.rawClientDataJSON))
        }
    }

    public func get(challenge: Data, rpId: String, allowCredentials: [String]?) async throws -> AssertionJSON {
        let provider = ASAuthorizationPlatformPublicKeyCredentialProvider(relyingPartyIdentifier: rpId)
        let request = provider.createCredentialAssertionRequest(challenge: challenge)
        if let allowCredentials, !allowCredentials.isEmpty {
            request.allowedCredentials = try allowCredentials.map {
                ASAuthorizationPlatformPublicKeyCredentialDescriptor(credentialID: try fromB64url($0))
            }
        }
        return try await perform(request) { credential in
            guard let assertion = credential as? ASAuthorizationPlatformPublicKeyCredentialAssertion else {
                throw AccountError.protocolViolation("the passkey returned no assertion")
            }
            return AssertionJSON(
                credentialId: b64url(assertion.credentialID),
                authenticatorData: b64url(assertion.rawAuthenticatorData),
                clientDataJson: b64url(assertion.rawClientDataJSON), signature: b64url(assertion.signature))
        }
    }

    @MainActor
    private func perform<T: Sendable>(
        _ request: ASAuthorizationRequest, _ extract: @escaping (ASAuthorizationCredential) throws -> T
    ) async throws -> T {
        let delegate = PasskeyDelegate()
        let controller = ASAuthorizationController(authorizationRequests: [request])
        controller.delegate = delegate
        controller.presentationContextProvider = delegate
        let credential = try await withCheckedThrowingContinuation { continuation in
            delegate.continuation = continuation
            delegate.retain = delegate
            controller.performRequests()
        }
        return try extract(credential)
    }
}

/// One ceremony's delegate: resumes once, with the credential or the error.
private final class PasskeyDelegate: NSObject, ASAuthorizationControllerDelegate,
    ASAuthorizationControllerPresentationContextProviding, @unchecked Sendable
{
    var continuation: CheckedContinuation<ASAuthorizationCredential, Error>?
    /// Kept alive until the ceremony ends (the controller holds its delegate weakly).
    var retain: PasskeyDelegate?

    func authorizationController(
        controller: ASAuthorizationController, didCompleteWithAuthorization authorization: ASAuthorization
    ) {
        continuation?.resume(returning: authorization.credential)
        continuation = nil
        retain = nil
    }

    func authorizationController(controller: ASAuthorizationController, didCompleteWithError error: Error) {
        if let error = error as? ASAuthorizationError, error.code == .canceled {
            continuation?.resume(throwing: PasskeyCancelled())
        } else {
            continuation?.resume(throwing: error)
        }
        continuation = nil
        retain = nil
    }

    func presentationAnchor(for controller: ASAuthorizationController) -> ASPresentationAnchor {
        MainActor.assumeIsolated {
            #if os(iOS)
            let scenes = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
            return scenes.flatMap(\.windows).first(where: \.isKeyWindow) ?? scenes.first?.windows.first
                ?? ASPresentationAnchor()
            #else
            return NSApplication.shared.keyWindow ?? ASPresentationAnchor()
            #endif
        }
    }
}
#endif
