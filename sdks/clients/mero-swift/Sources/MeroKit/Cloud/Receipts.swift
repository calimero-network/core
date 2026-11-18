import CryptoKit
import Foundation

#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

// Checking a pipeline run's receipt as core's `calimero-client` does
// (`Client::verify_app_cluster`, `attest::verify_receipt_key`,
// `Client::verify_receipt`): the receipt is signed by the answering node's
// receipt key, and that key is one every replica's quote attests, bound to the
// cluster and node the entry node names. A quote of the unbound
// `receipt-key.v2` form is never accepted. A version 2 receipt also proves
// each job's report: its job key signed what the proof says, and a job node's
// quote under the client's policy binds that key, node, cluster and cluster
// identity. Port of mero-js `client/receipts.ts`.

/// A receipt, or an attestation it rests on, that does not check out.
/// (mero-js throws a plain `Error` with the same messages.)
public struct ReceiptError: Error, LocalizedError, Sendable, Equatable, CustomStringConvertible {
    public let message: String

    public init(_ message: String) { self.message = message }

    public var errorDescription: String? { message }
    public var description: String { message }
}

/// A file named by id, size and plaintext SHA-256 (hex). (== mero-js `FileRef`.)
public struct ReceiptFileRef: Codable, Sendable, Equatable {
    public var id: String
    public var size: UInt64
    public var sha256: String
}

/// A run's resource use, as decimal strings. (== mero-js `StepRunProof.usage`.)
public struct ReceiptUsage: Codable, Sendable, Equatable {
    public var cpuMicros: String
    public var gibMicros: String
    public var fuel: String
}

/// A file as a job's report signs it: id, plaintext size and SHA-256, and
/// each ciphertext segment's SHA-256 (lowercase hex). (== mero-js `JobOutput`.)
public struct JobOutput: Codable, Sendable, Equatable {
    public var id: String
    public var size: UInt64
    public var sha256: String
    public var segments: [String]

    public init(id: String, size: UInt64, sha256: String, segments: [String]) {
        self.id = id
        self.size = size
        self.sha256 = sha256
        self.segments = segments
    }
}

/// The rest of what a job's report signs: the result's hash, the failure
/// text, each output's segment hashes, the module log. (== mero-js `JobEvidence`.)
public struct JobEvidence: Codable, Sendable, Equatable {
    public var resultSha256: String
    public var message: String?
    public var segments: [[String]]
    public var log: JobOutput?
}

/// What one step run's proof records: which job node ran which assignment,
/// and what came out. (== mero-js `StepRunProof`.)
public struct StepRunProof: Codable, Sendable, Equatable {
    public var run: String
    public var step: String
    public var index: UInt32
    public var outcome: String
    public var attempt: UInt32
    public var node: UInt64?
    public var jobKey: String?
    public var assignmentDigest: String?
    public var reportSha256: String?
    public var signature: String?
    public var outputs: [ReceiptFileRef]
    public var usage: ReceiptUsage
    /// What the job's report signs that the fields above do not (version 2 only).
    public var job: JobEvidence?
}

/// A run answered from the step cache: it ran nothing, so it has no proof. (== mero-js `CachedRun`.)
public struct CachedRun: Codable, Sendable, Equatable {
    public var index: UInt32
    public var cached: Bool
    public var cacheKey: String
}

/// One run of a step: cached, or proven. (Core's untagged `RunEntry`.)
public enum ReceiptRun: Codable, Sendable, Equatable {
    case cached(CachedRun)
    case proof(StepRunProof)

    public init(from decoder: Decoder) throws {
        if let cached = try? CachedRun(from: decoder) {
            self = .cached(cached)
        } else {
            self = .proof(try StepRunProof(from: decoder))
        }
    }

    public func encode(to encoder: Encoder) throws {
        switch self {
        case .cached(let run): try run.encode(to: encoder)
        case .proof(let run): try run.encode(to: encoder)
        }
    }

    var index: UInt32 {
        switch self {
        case .cached(let run): return run.index
        case .proof(let run): return run.index
        }
    }
}

/// One step of a receipt. (== mero-js `StepReceipt`.)
public struct StepReceipt: Codable, Sendable, Equatable {
    public var status: String
    public var reason: String?
    public var runs: [ReceiptRun]
}

/// What the node states about a finished pipeline run (core's
/// `PipelineReceipt`), as parsed from the bytes its signature covers.
/// `cluster` is the cluster identity key, hex. (== mero-js `PipelineReceipt`.)
public struct PipelineReceipt: Codable, Sendable, Equatable {
    public var version: UInt32
    public var cluster: String
    public var app: String
    public var run: String
    public var pipeline: String
    public var release: String
    public var pipelineHash: String
    public var modules: [String: String]
    public var inputs: [String: [ReceiptFileRef]]
    public var params: [String: JSONValue]
    public var state: String
    public var startTsMs: UInt64
    public var endTsMs: UInt64
    public var steps: [String: StepReceipt]
    public var costSoFar: String?
    /// Each job key a proof names, its job node's attestation (version 2 only).
    public var jobAttestations: [String: JSONValue]?
}

/// Every replica of an app, attested: its receipt key by node id, the
/// governance they all attest (its canonical fields), and the cluster name
/// every quote bound. (== mero-js `AttestedCluster`.)
public struct AttestedCluster: Sendable, Equatable {
    public var keys: [UInt64: Data]
    public var governance: [String: String]
    public var cluster: String
}

/// One job report's signed fields, in the order ``reportMessage(_:)`` writes
/// them (core's `ReportFields`). (== mero-js `ReportFields`.)
public struct ReportFields: Codable, Sendable, Equatable {
    /// SHA-256 of the assignment message the job ran (hex).
    public var assignment: String
    public var app: String
    public var run: String
    public var attempt: UInt32
    /// `nil` for a done report, else why it failed.
    public var reason: String?
    public var message: String
    public var resultSha256: String
    public var outputs: [JobOutput]
    /// CPU micros, GiB micros, fuel, as decimal strings.
    public var usage: [String]
    public var log: JobOutput?

    public init(
        assignment: String, app: String, run: String, attempt: UInt32, reason: String?, message: String,
        resultSha256: String, outputs: [JobOutput], usage: [String], log: JobOutput?
    ) {
        self.assignment = assignment
        self.app = app
        self.run = run
        self.attempt = attempt
        self.reason = reason
        self.message = message
        self.resultSha256 = resultSha256
        self.outputs = outputs
        self.usage = usage
        self.log = log
    }
}

// MARK: - Bindings

private let pipelineReceiptDomain = Data("calimero.pipeline.receipt.v1\0".utf8)
private let jobReportDomain = Data("calimero.compute.report.v1\0".utf8)
private let jobReportLogDomain = Data("calimero.compute.report.log.v1\0".utf8)
private let jobKeyDomain = Data("calimero.tee-attest.job-key.v1".utf8)
private let receiptKeyDomain = Data("calimero.tee-attest.receipt-key.v3".utf8)
private let receiptVersionJobs: UInt32 = 2
private let jobFailReasons: Set<String> = ["fuel", "memory", "trap", "app-error", "integrity", "cancelled"]
private let leaderFailReasons: Set<String> = ["lost", "refused", "timeout", "quota", "not-granted", "too-large"]

private func prefixed(_ text: String) -> Data {
    let bytes = Data(text.utf8)
    return BigEndian.u32(UInt32(bytes.count)) + bytes
}

/// The bytes a receipt-key quote binds of the governance: its JSON, fields in
/// declaration order, the later ones only when not their default (core's
/// `Governance::canonical`). (== mero-js `governanceCanonical`.)
public func governanceCanonical(_ governance: JSONValue) throws -> Data {
    guard case .object(let g) = governance else { throw ReceiptError("the governance is not an object") }
    func one(_ label: String, _ allowed: [String], _ fallback: String? = nil) throws -> String {
        if g[label] == nil, let fallback { return fallback }
        guard case .string(let value)? = g[label], allowed.contains(value) else {
            throw ReceiptError("the governance's \(label) is not one of \(allowed.joined(separator: ", "))")
        }
        return value
    }
    var fields: [(String, String)] = [
        ("releases", try one("releases", ["owner", "operator"])),
        ("appUpgrades", try one("appUpgrades", ["owner", "operator"])),
        ("outbound", try one("outbound", ["owner", "operator"])),
    ]
    let deployments = try one("deployments", ["open", "granted"], "open")
    if deployments != "open" { fields.append(("deployments", deployments)) }
    let deployers = try one("deployers", ["operator", "owner"], "operator")
    if deployers != "operator" { fields.append(("deployers", deployers)) }
    let signUp = try one("signUp", ["open", "invite", "closed"], "open")
    if signUp != "open" { fields.append(("signUp", signUp)) }
    return Data(("{" + fields.map { "\"\($0.0)\":\"\($0.1)\"" }.joined(separator: ",") + "}").utf8)
}

/// What a receipt-key quote binds after its nonce (core's
/// `receipt_key_binding`, `receipt-key.v3`): the key, the cluster's
/// governance, and the cluster (u32-BE length-prefixed) and node id (u64 BE).
/// (== mero-js `receiptKeyBinding`, as bytes.)
public func receiptKeyBinding(_ receiptKey: Data, governance: JSONValue, cluster: String, node: UInt64) throws -> Data {
    let g = sha256(try governanceCanonical(governance))
    return sha256(receiptKeyDomain + receiptKey + g + prefixed(cluster) + BigEndian.u64(node))
}

/// What a job-node attestation's quote binds after the nonce (core's
/// `job_key_binding`): both keys, the release and cluster (length-prefixed),
/// the node id, and the cluster identity when there is one.
/// (== mero-js `jobKeyBinding`, as bytes.)
public func jobKeyBinding(
    transportKey: Data, jobKey: Data, release: String, cluster: String, node: UInt64, clusterIdentity: Data?
) -> Data {
    sha256(
        jobKeyDomain + transportKey + jobKey + prefixed(release) + prefixed(cluster) + BigEndian.u64(node)
            + (clusterIdentity ?? Data()))
}

/// The 32 bytes of a hash spelled as the wire spells it: 64 lowercase hex digits (core's `hash_bytes`).
private func hashBytes(_ text: String) throws -> Data {
    guard text.utf8.count == 64, text.utf8.allSatisfy(Hex.isLowerHexDigit),
        let bytes = try? Hex.decode(text, label: "hash", bytes: 32)
    else { throw ReceiptError("a hash is 64 lowercase hex digits") }
    return bytes
}

/// Hex as core's `hex::decode` reads it (either case), of exactly `bytes` bytes: no trimming, nothing else.
private func strictHex(_ value: JSONValue?, _ label: String, _ bytes: Int) throws -> Data {
    guard case .string(let text)? = value, text.utf8.count == bytes * 2,
        text.utf8.allSatisfy({ Hex.isLowerHexDigit($0) || (0x41...0x46).contains($0) }),
        let data = try? Hex.decode(text, label: label, bytes: bytes)
    else { throw ReceiptError("\(label) is not \(bytes) bytes of hex") }
    return data
}

/// A node id: a non-negative safe integer, as a JSON number.
private func nodeId(_ value: JSONValue?, _ label: String) throws -> UInt64 {
    guard case .number(let n)? = value, n >= 0, n <= 9_007_199_254_740_991, n == n.rounded() else {
        throw ReceiptError("\(label) is not a node id")
    }
    return UInt64(n)
}

/// A decimal `u64` as the receipt spells usage: digits only.
private func decimal(_ text: String) throws -> UInt64 {
    guard !text.isEmpty, text.utf8.allSatisfy({ (0x30...0x39).contains($0) }), let n = UInt64(text) else {
        throw ReceiptError("\(CanonicalJSON.quote(text)) is not a decimal integer")
    }
    return n
}

private func outputBytes(_ o: JobOutput) throws -> Data {
    var out = try hashBytes(o.id) + BigEndian.u64(o.size) + hashBytes(o.sha256)
    out += BigEndian.u32(UInt32(o.segments.count))
    for segment in o.segments { out += try hashBytes(segment) }
    return out
}

/// The bytes a job node signs for one report (core's `report_message`): the
/// domain, the assignment digest, the length-prefixed app, run, reason and
/// message, the result's hash, the outputs, the usage, then the log after its
/// own domain; integers big-endian, hashes as their 32 bytes.
/// (== mero-js `reportMessage`.)
public func reportMessage(_ f: ReportFields) throws -> Data {
    guard f.usage.count == 3 else { throw ReceiptError("usage is three numbers") }
    var out = jobReportDomain + (try hashBytes(f.assignment))
    out += prefixed(f.app) + prefixed(f.run) + BigEndian.u32(f.attempt)
    out += Data([f.reason == nil ? 0 : 1]) + prefixed(f.reason ?? "") + prefixed(f.message)
    out += try hashBytes(f.resultSha256)
    out += BigEndian.u32(UInt32(f.outputs.count))
    for output in f.outputs { out += try outputBytes(output) }
    for n in f.usage { out += BigEndian.u64(try decimal(n)) }
    if let log = f.log { out += jobReportLogDomain + (try outputBytes(log)) }
    return out
}

/// The bytes a receipt's signature covers (core's `receipt_message`): the
/// domain, then the receipt's canonical JSON. (== mero-js `receiptMessage`.)
public func receiptMessage(_ receipt: JSONValue) -> Data {
    pipelineReceiptDomain + Data(ReceiptJCS.string(receipt).utf8)
}

// MARK: - Verifying

/// Checks a receipt-key attestation `doc` as core's `verify_receipt_key`
/// does: its quote binds the key, the governance and the `cluster` and
/// `node` the caller asked for (never the document's own fields, which
/// nothing signs). Answers the key and the governance's canonical fields.
/// (== mero-js `verifyReceiptKey`.)
public func verifyReceiptKey(
    _ doc: JSONValue, nonce: Data, cluster: String, node: UInt64, policy: CloudPolicy,
    session: URLSession = .shared
) async throws -> (key: Data, governance: [String: String]) {
    guard case .string(let quote)? = doc["quote"] else { throw ReceiptError("the attestation carries no quote") }
    guard case .string(let receiptKey)? = doc["receiptKey"], !receiptKey.isEmpty else {
        throw ReceiptError("the attestation names no receipt key")
    }
    guard let governance = doc["governance"], case .object = governance else {
        throw ReceiptError("the attestation names no governance")
    }
    let key = try strictHex(.string(receiptKey), "the receipt key", 32)
    let suffix = try receiptKeyBinding(key, governance: governance, cluster: cluster, node: node)
    try await verifyQuoteBinding(
        quoteDoc(quote, doc), nonce: nonce, suffix: suffix, policy: policy, what: "receipt key", session: session)
    let canonical = try governanceCanonical(governance)
    return (key, try MeroJSON.decode([String: String].self, from: canonical))
}

/// The document `verifyQuoteBinding` judges: the quote and the release it names.
private func quoteDoc(_ quote: String, _ doc: JSONValue) -> AttestationDoc {
    var release: AttestationDoc.Release?
    if case .string(let version)? = doc["release"]?["version"], case .string(let profile)? = doc["release"]?["profile"]
    {
        release = AttestationDoc.Release(version: version, profile: profile)
    }
    return AttestationDoc(quote: quote, transportKey: "", release: release)
}

private func message(_ error: Error) -> String {
    (error as? LocalizedError)?.errorDescription ?? "\(error)"
}

/// Verifies every replica of `app` (core's `Client::verify_app_cluster`):
/// each one's quote over a fresh nonce, held to `policy`, binding its receipt
/// key, the cluster's governance, and the cluster and node id the entry node
/// names. Any replica error, a node id that differs from the document's, or
/// replicas that attest other governance fails the whole check. `fetch`
/// answers `GET /apps/{app}/attestation?nonce=` for a nonce (hex).
func attestAppCluster(
    _ app: String, policy: CloudPolicy, session: URLSession,
    fetch: @Sendable (String) async throws -> JSONValue
) async throws -> AttestedCluster {
    let nonce = randomBytes(32)
    let answer = try await fetch(Hex.encode(nonce))
    guard case .string(let cluster)? = answer["cluster"], case .array(let replicas)? = answer["replicas"] else {
        throw ReceiptError("the node answered a malformed attestation")
    }
    var keys: [UInt64: Data] = [:]
    var governance: [String: String]?
    for replica in replicas {
        guard case .object(let entry) = replica else { throw ReceiptError("a replica entry is not an object") }
        let node = try nodeId(entry["node"], "a replica")
        if let error = entry["error"] {
            let text: String
            if case .string(let s) = error { text = s } else { text = CanonicalJSON.string(error) }
            throw ReceiptError("replica \(node) gave no attestation: \(text)")
        }
        guard let doc = entry["attestation"], case .object = doc else {
            throw ReceiptError("replica \(node) gave neither an attestation nor an error")
        }
        let docNode = try nodeId(doc["node"], "replica \(node)'s document")
        guard docNode == node else { throw ReceiptError("replica \(node) answered as node \(docNode)") }
        let attested: (key: Data, governance: [String: String])
        do {
            attested = try await verifyReceiptKey(
                doc, nonce: nonce, cluster: cluster, node: node, policy: policy, session: session)
        } catch {
            throw ReceiptError("replica \(node): \(message(error))")
        }
        if let governance, governance != attested.governance {
            throw ReceiptError("replica \(node) attests other governance")
        }
        governance = attested.governance
        keys[node] = attested.key
    }
    guard let governance else { throw ReceiptError("no replica of \(app) attested") }
    return AttestedCluster(keys: keys, governance: governance, cluster: cluster)
}

/// Whether `answer` (`GET /apps/{app}/pipeline-runs/{run}/receipt`'s
/// `{ receipt, node, signature }`) is signed by `key` (core's
/// `verify_pipeline_receipt`). Answers the receipt as parsed from the
/// canonical bytes that were verified, checked against core's receipt
/// schema. (== mero-js `verifyReceiptSignature`.)
public func verifyReceiptSignature(_ answer: JSONValue, key: Data) throws -> PipelineReceipt {
    guard case .object(let fields) = answer else { throw ReceiptError("the receipt answer is not an object") }
    if let unknown = fields.keys.sorted().first(where: { !["receipt", "node", "signature"].contains($0) }) {
        throw ReceiptError("the receipt answer's \(unknown) is not a known field")
    }
    let signature = try strictHex(fields["signature"], "the signature", 64)
    guard let receipt = fields["receipt"] else { throw ReceiptError("the receipt answer carries no receipt") }
    let canonical = Data(ReceiptJCS.string(receipt).utf8)
    guard Ed25519.verify(publicKey: key, signature: signature, message: pipelineReceiptDomain + canonical) else {
        throw ReceiptError("the receipt is not signed by that key")
    }
    let parsed: JSONValue
    do {
        parsed = try MeroJSON.decode(JSONValue.self, from: canonical)
    } catch {
        throw ReceiptError("the receipt is not JSON")
    }
    try ReceiptSchema.pipelineReceipt(parsed, "receipt")
    do {
        return try MeroJSON.decode(PipelineReceipt.self, from: canonical)
    } catch {
        throw ReceiptError("the receipt does not read as a pipeline receipt")
    }
}

/// The reason a job signed for `outcome`: `.signed(nil)` when done, or
/// `nil` for an end only the leader gives.
private func signedReason(_ outcome: String, reported: Bool) throws -> String?? {
    if outcome == "done" { return .some(nil) }
    if outcome == "cancelled" { return reported ? .some("cancelled") : nil }
    let bad = ReceiptError("the outcome \(CanonicalJSON.quote(outcome)) is not one a run ends with")
    guard outcome.hasPrefix("failed:") else { throw bad }
    let reason = String(outcome.dropFirst("failed:".count))
    if jobFailReasons.contains(reason) { return .some(reason) }
    guard leaderFailReasons.contains(reason) else { throw bad }
    return nil
}

/// Checks one step run's proof (core's `check_proof`): it is the run the
/// receipt's run, step and index name, and either the leader ended it (no
/// report, outputs or usage) or its job node signed a report that says
/// exactly what the proof says. Answers the job key and node of a signed one.
private func checkProof(
    _ receipt: PipelineReceipt, _ step: String, _ p: StepRunProof
) throws -> (key: String, node: UInt64)? {
    let expected = "\(receipt.run).\(step).\(p.index)"
    guard p.run == expected, p.step == step else {
        throw ReceiptError("the proof of \(p.run) is listed as \(expected)")
    }
    let reported = p.signature != nil
    guard let signed = try signedReason(p.outcome, reported: reported) else {
        let zero = [p.usage.cpuMicros, p.usage.gibMicros, p.usage.fuel].allSatisfy { $0 == "0" }
        if reported || p.reportSha256 != nil || p.job != nil || !p.outputs.isEmpty || !zero {
            throw ReceiptError("\(p.run) ended \(p.outcome) by the leader, yet names a report, outputs or usage")
        }
        return nil
    }
    func missing(_ what: String) -> ReceiptError {
        ReceiptError("\(p.run) ended \(p.outcome) but its proof has no \(what)")
    }
    guard let node = p.node else { throw missing("job node") }
    guard let jobKey = p.jobKey else { throw missing("job key") }
    guard let assignment = p.assignmentDigest else { throw missing("assignment digest") }
    guard let reportSha256 = p.reportSha256 else { throw missing("report hash") }
    guard let signature = p.signature else { throw missing("report signature") }
    guard let job = p.job else { throw missing("job evidence (it ended before receipts carried it)") }
    guard job.segments.count == p.outputs.count else {
        throw ReceiptError(
            "\(p.run): the job evidence lists segments for \(job.segments.count) outputs, the proof \(p.outputs.count)")
    }
    let outputs = zip(p.outputs, job.segments).map {
        JobOutput(id: $0.id, size: $0.size, sha256: $0.sha256, segments: $1)
    }
    let report: Data
    do {
        report = try reportMessage(
            ReportFields(
                assignment: assignment, app: receipt.app, run: p.run, attempt: p.attempt, reason: signed,
                message: job.message ?? "", resultSha256: job.resultSha256, outputs: outputs,
                usage: [p.usage.cpuMicros, p.usage.gibMicros, p.usage.fuel], log: job.log))
    } catch {
        throw ReceiptError("\(p.run): \(message(error))")
    }
    guard Hex.encode(sha256(report)) == reportSha256 else {
        throw ReceiptError("\(p.run): the report hash is not that of what the proof says")
    }
    let key = try hashBytes(jobKey)
    guard signature.utf8.count == 128, let sig = try? Hex.decode(signature, label: "signature", bytes: 64),
        !signature.contains(where: \.isWhitespace)
    else { throw ReceiptError("\(p.run): the report signature is not 64 bytes of hex") }
    guard Ed25519Strict.verify(publicKey: key, message: report, signature: sig) else {
        throw ReceiptError("\(p.run): the job key did not sign what the proof says")
    }
    return (jobKey, node)
}

/// Checks every step run's proof of a version 2 receipt (core's
/// `verify_job_reports`), each step's runs listed once in index order, and
/// answers each job key that signed a report with the node it ran on; a key
/// named on two nodes is refused. Whose key each is (its attestation) is for
/// the caller to verify. What a job signed is proven; the rest (step and run
/// states, cached runs, and the node and key a leader-ended run names) is the
/// signing replica's word. (== mero-js `verifyJobReports`.)
public func verifyJobReports(_ receipt: PipelineReceipt) throws -> [String: UInt64] {
    guard receipt.version == receiptVersionJobs else {
        throw ReceiptError(
            "the receipt is version \(receipt.version); only version \(receiptVersionJobs) carries what each job signed"
        )
    }
    var keys: [String: UInt64] = [:]
    for step in receipt.steps.keys.sorted() {
        for (i, entry) in (receipt.steps[step]?.runs ?? []).enumerated() {
            guard entry.index == i else {
                throw ReceiptError("step \(step) lists run \(entry.index) where run \(i) belongs")
            }
            switch entry {
            case .cached(let run):
                guard run.cached else { throw ReceiptError("step \(step) run \(i) is listed as not cached") }
            case .proof(let proof):
                guard let proved = try checkProof(receipt, step, proof) else { continue }
                if let other = keys[proved.key], other != proved.node {
                    throw ReceiptError("job key \(proved.key) is named on two nodes")
                }
                keys[proved.key] = proved.node
            }
        }
    }
    return keys
}

/// Checks that `receipt` carries the attestation of job key `key`: a job
/// node's quote, under `policy`, binding that key to node `node` of
/// `cluster` taking work from the cluster identity the receipt names.
private func verifyJobAttestation(
    _ receipt: PipelineReceipt, cluster: String, key: String, node: UInt64, policy: CloudPolicy,
    session: URLSession
) async throws {
    guard let doc = receipt.jobAttestations?[key] else {
        throw ReceiptError("the receipt carries no attestation for it")
    }
    guard doc["jobKey"] == .string(key) else { throw ReceiptError("its attestation is another key's") }
    let docNode = doc["node"]?.doubleValue
    let docCluster = doc["cluster"]?.stringValue
    guard docNode == Double(node), docCluster == cluster else {
        throw ReceiptError(
            "it is attested as node \(docNode.map { ReceiptJCS.number($0) } ?? "undefined") of cluster "
                + "\(docCluster ?? "undefined"), not node \(node) of \(cluster)")
    }
    guard doc["clusterIdentity"] == .string(receipt.cluster) else {
        throw ReceiptError("its job node takes work from another cluster identity")
    }
    let nonce: Data
    do {
        nonce = try hashBytes(doc["nonce"]?.stringValue ?? "")
    } catch {
        throw ReceiptError("its attestation's nonce is not 32 bytes of hex")
    }
    let version = doc["release"]?["version"]?.stringValue ?? ""
    let binding = jobKeyBinding(
        transportKey: try strictHex(doc["transportKey"], "the transport key", 32),
        jobKey: try Hex.decode(key, label: "the job key", bytes: 32), release: version, cluster: cluster,
        node: node, clusterIdentity: try strictHex(doc["clusterIdentity"], "the cluster identity", 32))
    try await verifyQuoteBinding(
        quoteDoc(doc["quote"]?.stringValue ?? "", doc), nonce: nonce, suffix: binding, policy: policy,
        what: "job key", session: session)
}

/// Checks that the node that answered holds a receipt key attested under
/// `policy` (`attested`), signed `answer` with it, that the receipt is for
/// `app` and `run`, that every job report it names is signed by its job key
/// (``verifyJobReports(_:)``), and that each such key is a job node's of the
/// attested cluster and the receipt's cluster identity, under `policy`. A
/// version 1 receipt is refused. Answers the receipt.
func checkPipelineReceipt(
    _ attested: AttestedCluster, app: String, run: String, answer: JSONValue, policy: CloudPolicy,
    session: URLSession
) async throws -> PipelineReceipt {
    guard case .object = answer else { throw ReceiptError("the receipt answer is not an object") }
    let node = try nodeId(answer["node"], "the receipt answer")
    guard let key = attested.keys[node] else { throw ReceiptError("node \(node) is not an attested replica of \(app)") }
    let receipt = try verifyReceiptSignature(answer, key: key)
    guard receipt.app == app else { throw ReceiptError("the receipt is for app \(receipt.app), not \(app)") }
    guard receipt.run == run else { throw ReceiptError("the receipt is for run \(receipt.run), not \(run)") }
    for (jobKey, jobNode) in try verifyJobReports(receipt).sorted(by: { $0.key < $1.key }) {
        do {
            try await verifyJobAttestation(
                receipt, cluster: attested.cluster, key: jobKey, node: jobNode, policy: policy, session: session)
        } catch {
            throw ReceiptError("job key \(jobKey): \(message(error))")
        }
    }
    return receipt
}

// MARK: - Ed25519 as `verify_strict`

/// Ed25519 as core's `verify_strict` reads it: a non-canonical S and a
/// small-order key or R are refused as well as a bad signature.
enum Ed25519Strict {
    /// The group order L, little-endian.
    private static let order: [UInt8] = [
        0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
    ]

    /// Every encoding of a point of order dividing 8 (its y read mod p, either sign).
    static let smallOrder: Set<Data> = {
        let ys = [
            "0100000000000000000000000000000000000000000000000000000000000000",  // identity
            "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",  // order 2
            "0000000000000000000000000000000000000000000000000000000000000000",  // order 4
            "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",  // order 8
            "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",  // order 8
            "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",  // p ≡ 0
            "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",  // p + 1 ≡ 1
        ]
        var out = Set<Data>()
        for y in ys {
            guard var bytes = try? Hex.decode(y, label: "point", bytes: 32) else { continue }
            out.insert(bytes)
            bytes[31] ^= 0x80
            out.insert(bytes)
        }
        return out
    }()

    static func isSmallOrder(_ encoded: Data) -> Bool { smallOrder.contains(Data(encoded)) }

    /// Whether the scalar S (little-endian) is below the group order.
    static func isCanonical(_ s: Data) -> Bool {
        let bytes = Array(s)
        for i in stride(from: 31, through: 0, by: -1) where bytes[i] != order[i] {
            return bytes[i] < order[i]
        }
        return false
    }

    static func verify(publicKey: Data, message: Data, signature: Data) -> Bool {
        guard publicKey.count == 32, signature.count == 64 else { return false }
        let r = Data(signature.prefix(32))
        let s = Data(signature.suffix(32))
        guard isCanonical(s), !isSmallOrder(publicKey), !isSmallOrder(r) else { return false }
        return Ed25519.verify(publicKey: publicKey, signature: signature, message: message)
    }
}

// MARK: - Canonical JSON (RFC 8785)

/// RFC 8785 canonical JSON, as core canonicalizes a receipt: every number
/// as an f64 written as ECMAScript writes it, object keys in UTF-16 order.
enum ReceiptJCS {
    static func string(_ value: JSONValue) -> String {
        var out = ""
        write(value, into: &out)
        return out
    }

    private static func write(_ value: JSONValue, into out: inout String) {
        switch value {
        case .null: out += "null"
        case .bool(let b): out += b ? "true" : "false"
        case .number(let n): out += number(n)
        case .string(let s): out += CanonicalJSON.quote(s)
        case .array(let items):
            out += "["
            for (i, item) in items.enumerated() {
                if i > 0 { out += "," }
                write(item, into: &out)
            }
            out += "]"
        case .object(let fields):
            out += "{"
            let keys = fields.keys.sorted { Array($0.utf16).lexicographicallyPrecedes(Array($1.utf16)) }
            for (i, key) in keys.enumerated() {
                if i > 0 { out += "," }
                out += CanonicalJSON.quote(key) + ":"
                write(fields[key] ?? .null, into: &out)
            }
            out += "}"
        }
    }

    /// `n` as ECMAScript's `Number.prototype.toString` writes it: the
    /// shortest digits that read back as `n`, in plain notation from 1e-6 up
    /// to 1e21, exponent notation outside.
    static func number(_ n: Double) -> String {
        if n == 0 || !n.isFinite { return "0" }
        if n < 0 { return "-" + number(-n) }
        // Swift's description is the shortest round-trip digits too, in its own layout.
        let text = "\(n)"
        let parts = text.split(separator: "e", maxSplits: 1)
        let exponent = parts.count > 1 ? Int(parts[1]) ?? 0 : 0
        let mantissa = parts[0].split(separator: ".", omittingEmptySubsequences: false)
        let whole = String(mantissa[0])
        var digits = Array(whole + (mantissa.count > 1 ? String(mantissa[1]) : ""))
        var point = whole.count + exponent
        while digits.first == "0", digits.count > 1 {
            digits.removeFirst()
            point -= 1
        }
        while digits.last == "0", digits.count > 1 { digits.removeLast() }
        let k = digits.count
        let ds = String(digits)
        if k <= point && point <= 21 { return ds + String(repeating: "0", count: point - k) }
        if 0 < point && point <= 21 { return String(digits[..<point]) + "." + String(digits[point...]) }
        if -6 < point && point <= 0 { return "0." + String(repeating: "0", count: -point) + ds }
        let e = point - 1
        let head = String(digits[0]) + (k > 1 ? "." + String(digits[1...]) : "")
        return head + "e" + (e < 0 ? "-" : "+") + String(abs(e))
    }
}

// MARK: - Core's receipt schema

/// The receipt's schema, as core's serde types read it: `deny_unknown_fields`,
/// required fields, typed values. Every refusal reads `the receipt's <path> …`.
enum ReceiptSchema {
    typealias Check = @Sendable (JSONValue, String) throws -> Void

    static func fail(_ path: String, _ what: String) -> ReceiptError {
        ReceiptError("the receipt's \(path) \(what)")
    }

    static let str: Check = { v, p in
        guard case .string = v else { throw fail(p, "is not a string") }
    }
    static let bool: Check = { v, p in
        guard case .bool = v else { throw fail(p, "is not a boolean") }
    }
    static let anything: Check = { _, _ in }
    static let obj: Check = { v, p in
        guard case .object = v else { throw fail(p, "is not an object") }
    }

    static func uint(_ max: Double) -> Check {
        { v, p in
            guard case .number(let n) = v, n == n.rounded(), n >= 0, n <= max else {
                throw fail(p, "is not an unsigned integer in range")
            }
        }
    }

    static let u32 = uint(4_294_967_295)
    // 2^64 itself is not a u64; the largest f64 below it is.
    static let u64: Check = { v, p in
        guard case .number(let n) = v, n == n.rounded(), n >= 0, n < 18_446_744_073_709_551_616.0 else {
            throw fail(p, "is not an unsigned integer in range")
        }
    }

    static func list(_ item: @escaping Check) -> Check {
        { v, p in
            guard case .array(let items) = v else { throw fail(p, "is not an array") }
            for (i, x) in items.enumerated() { try item(x, "\(p)[\(i)]") }
        }
    }

    static func map(_ value: @escaping Check) -> Check {
        { v, p in
            guard case .object(let fields) = v else { throw fail(p, "is not an object") }
            for key in fields.keys.sorted() { try value(fields[key] ?? .null, "\(p).\(key)") }
        }
    }

    /// `optional` fields are Options (null reads as absent); `defaulted` ones
    /// may be absent but never null; an `open` struct allows other fields, as
    /// a serde type without `deny_unknown_fields`.
    static func structure(
        _ required: KeyValuePairs<String, Check>, optional: KeyValuePairs<String, Check> = [:],
        defaulted: KeyValuePairs<String, Check> = [:], open: Bool = false
    ) -> Check {
        let known = Set(required.map(\.key) + optional.map(\.key) + defaulted.map(\.key))
        return { v, p in
            guard case .object(let fields) = v else { throw fail(p, "is not an object") }
            if !open, let unknown = fields.keys.sorted().first(where: { !known.contains($0) }) {
                throw fail("\(p).\(unknown)", "is not a known field")
            }
            for (key, check) in required {
                guard let value = fields[key] else { throw fail("\(p).\(key)", "is missing") }
                try check(value, "\(p).\(key)")
            }
            for (key, check) in optional {
                if let value = fields[key], value != .null { try check(value, "\(p).\(key)") }
            }
            for (key, check) in defaulted {
                if let value = fields[key] { try check(value, "\(p).\(key)") }
            }
        }
    }

    static let fileRef = structure(["id": str, "size": u64, "sha256": str])
    static let jobOutput = structure(["id": str, "size": u64, "sha256": str, "segments": list(str)])
    static let jobEvidence = structure(
        ["resultSha256": str, "segments": list(list(str))], optional: ["log": jobOutput], defaulted: ["message": str])
    static let attestationDoc = structure(
        [
            "quote": str, "transportKey": str, "appHash": str, "nonce": str,
            "release": structure(["version": str, "profile": str], open: true), "cluster": str, "node": u64,
            "mock": bool,
        ],
        optional: ["receiptKey": str, "governance": obj, "kms": obj, "jobKey": str, "clusterIdentity": str],
        open: true)
    static let stepRun = structure(
        [
            "run": str, "step": str, "index": u32, "outcome": str, "attempt": u32, "outputs": list(fileRef),
            "usage": structure(["cpuMicros": str, "gibMicros": str, "fuel": str]),
        ],
        optional: [
            "node": u64, "jobKey": str, "assignmentDigest": str, "reportSha256": str, "signature": str,
            "job": jobEvidence,
        ])
    static let cachedRun = structure(["index": u32, "cached": bool, "cacheKey": str])
    // Core's `RunEntry` is untagged: a cached run first, else a step run proof.
    static let runEntry: Check = { v, p in
        do {
            try cachedRun(v, p)
        } catch {
            try stepRun(v, p)
        }
    }
    static let pipelineReceipt = structure(
        [
            "version": u32, "cluster": str, "app": str, "run": str, "pipeline": str, "release": str,
            "pipelineHash": str, "modules": map(str), "inputs": map(list(fileRef)), "params": map(anything),
            "state": str, "startTsMs": u64, "endTsMs": u64,
            "steps": map(structure(["status": str, "runs": list(runEntry)], optional: ["reason": str])),
        ],
        optional: ["costSoFar": str],
        defaulted: ["jobAttestations": map(attestationDoc)])
}
