import Foundation

@testable import MeroKit

// The fake nodes here answer what each test needs, often less than the
// document's shapes require: these read the bytes a facade call got back,
// whatever the generated type makes of them (`answerBytes`, internal), as
// JSON or as one of MeroKit's own types.
extension CalimeroClient {
    func answerJSON<R: Sendable>(_ operation: @Sendable () async throws -> R) async throws -> JSONValue {
        let data = try await answerBytes(operation)
        return try MeroJSON.decode(JSONValue.self, from: data.isEmpty ? Data("{}".utf8) : data)
    }

    func answer<T: Decodable, R: Sendable>(
        as type: T.Type, _ operation: @Sendable () async throws -> R
    ) async throws
        -> T
    {
        let data = try await answerBytes(operation)
        return try MeroJSON.decode(type, from: data.isEmpty ? Data("{}".utf8) : data)
    }
}
