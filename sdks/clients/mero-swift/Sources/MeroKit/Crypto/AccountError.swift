import Foundation

/// Errors the app layer raises itself, before or instead of asking a node.
/// Refusals a node answered are ``CalimeroError``s; a call while signed out
/// is ``SignedOutError`` (0.x: `AccountError.noRelay`).
public enum AccountError: Error, Equatable, Sendable {
    /// An input was malformed before anything was sent (wrong hex width, JSON
    /// that does not parse, …). Nothing was spent.
    case invalidInput(String)

    /// A node answered with something this client cannot use.
    case protocolViolation(String)

    /// The operation needs a signed-in session that is absent.
    case notSignedIn(String)
}

extension AccountError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .invalidInput(let m): return m
        case .protocolViolation(let m): return m
        case .notSignedIn(let m): return m
        }
    }
}
