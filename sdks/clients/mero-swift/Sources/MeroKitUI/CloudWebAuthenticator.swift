import Foundation
import MeroKit

/// Opens a web page in the system browser (the node's consent page) and
/// returns the URL it redirected back to.
///
/// The production implementation is ``SystemWebAuthenticator`` (the system
/// auth sheet, `ASWebAuthenticationSession`). Tests and UI-test builds inject
/// one that answers without a browser.
public protocol WebAuthenticating: Sendable {
    /// Present `url`; resolve with the callback URL (fragment included).
    func authenticate(url: URL, callback: CloudCallback) async throws -> URL
}

/// Where a page in the system browser (the consent page) sends the person
/// back to: the consent request's `returnTo`.
public enum CloudCallback: Sendable, Equatable {
    /// An app scheme, e.g. `mero-sample` → `mero-sample://enrol`. Works on every
    /// iOS version; the node must accept it as a `returnTo` (one the package's
    /// manifest names).
    case scheme(String, path: String = "enrol")
    /// A verified https callback (Associated Domains), iOS 17.4+ / macOS 14.4+.
    case https(host: String, path: String)

    /// The callback as a URL string (a consent request's `returnTo`).
    public var url: String {
        switch self {
        case .scheme(let scheme, let path): return "\(scheme)://\(path)"
        case .https(let host, let path): return "https://\(host)\(path.hasPrefix("/") ? path : "/" + path)"
        }
    }
}

/// The person dismissed the sheet.
public struct WebAuthenticationCancelled: Error, LocalizedError, Sendable {
    public init() {}
    public var errorDescription: String? { "The page was dismissed." }
}

#if canImport(AuthenticationServices) && (os(iOS) || os(macOS))
import AuthenticationServices
#if os(iOS)
import UIKit
#else
import AppKit
#endif

/// `ASWebAuthenticationSession`: the system sheet, like "Sign in with Google".
///
/// Not ephemeral by default, so the consent page's own session cookie (if any)
/// survives between approvals.
public final class SystemWebAuthenticator: NSObject, WebAuthenticating,
    ASWebAuthenticationPresentationContextProviding, @unchecked Sendable
{
    private var session: ASWebAuthenticationSession?
    private let prefersEphemeral: Bool

    public init(prefersEphemeralWebBrowserSession: Bool = false) {
        self.prefersEphemeral = prefersEphemeralWebBrowserSession
    }

    public func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
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

    public func authenticate(url: URL, callback: CloudCallback) async throws -> URL {
        try await present(url: url, callback: callback)
    }

    @MainActor
    private func present(url: URL, callback: CloudCallback) async throws -> URL {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<URL, Error>) in
            let completion: @Sendable (URL?, Error?) -> Void = { url, error in
                if let url { return continuation.resume(returning: url) }
                if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
                    return continuation.resume(throwing: WebAuthenticationCancelled())
                }
                continuation.resume(throwing: error ?? WebAuthenticationCancelled())
            }
            let session: ASWebAuthenticationSession
            switch callback {
            case .scheme(let scheme, _):
                session = ASWebAuthenticationSession(
                    url: url, callbackURLScheme: scheme, completionHandler: completion)
            case .https(let host, let path):
                guard #available(iOS 17.4, macOS 14.4, *) else {
                    return continuation.resume(
                        throwing: AccountError.invalidInput(
                            "https callbacks need iOS 17.4 / macOS 14.4; use an app scheme"))
                }
                session = ASWebAuthenticationSession(
                    url: url, callback: .https(host: host, path: path), completionHandler: completion)
            }
            session.presentationContextProvider = self
            session.prefersEphemeralWebBrowserSession = prefersEphemeral
            self.session = session
            if !session.start() {
                continuation.resume(
                    throwing: AccountError.protocolViolation("could not start the web auth session"))
            }
        }
    }
}
#endif
