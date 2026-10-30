//! What a leader and a job node say to each other: the assignment of a compute run and a
//! component, both signed by the cluster identity key, and the report of a run's end, signed by
//! the job node. The leader and the job node both build and check them here, so the signed
//! bytes are defined once.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::types::Caller;

const ASSIGNMENT_LABEL: &[u8] = b"calimero.compute.assignment.v1\0"; // prefix of the signed assignment
const BUNDLE_LABEL: &[u8] = b"calimero.compute.bundle.v1\0"; // prefix of the signed component hash
const REPORT_LABEL: &[u8] = b"calimero.compute.report.v1\0"; // prefix of the signed report message
const SHA256_LEN: usize = 32; // bytes of a SHA-256
pub const BUNDLE_SIGNATURE_HEADER: &str = "x-bundle-signature"; // the header carrying the cluster identity's signature over a component's hash
const MICROS_PER_SECOND: u128 = 1_000_000; // to turn GiB-microseconds into GiB-seconds
const BYTES_PER_GIB: u128 = 1 << 30; // a GiB, the unit of memory billed

/// Whether `text` is a SHA-256 as the wire spells it: 64 lowercase hex digits.
#[must_use]
pub fn is_hash(text: &str) -> bool {
    text.len() == 2 * SHA256_LEN && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The 32 bytes of a hash spelled as [`is_hash`] says.
pub(crate) fn hash_bytes(text: &str) -> EyreResult<[u8; SHA256_LEN]> {
    if !is_hash(text) {
        bail!("a hash is 64 lowercase hex digits");
    }
    hex::decode(text)?
        .try_into()
        .map_err(|_| eyre!("a hash is 32 bytes"))
}

mod b64 {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(
        clippy::ptr_arg,
        reason = "serde's `with` passes the field by reference"
    )]
    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD.decode(text).map_err(D::Error::custom)
    }
}

/// What a run may use, as the method's ABI declares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBudget {
    pub fuel: u64,
    pub memory_mib: u32,
    pub wall_secs: u32,
}

/// One input file: its mark, and its data key wrapped to the job node.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunInput {
    /// The blob id: hex SHA-256 of the ciphertext.
    pub id: String,
    /// Plaintext bytes.
    pub size: u64,
    /// Hex SHA-256 of the plaintext.
    pub sha256: String,
    #[serde(with = "b64")]
    pub wrapped: Vec<u8>,
}

impl std::fmt::Debug for RunInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunInput")
            .field("id", &self.id)
            .field("size", &self.size)
            .field("wrapped", &format_args!("<{} bytes>", self.wrapped.len()))
            .finish_non_exhaustive()
    }
}

/// One attempt of a run, as the cluster hands it to a job node.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Assignment {
    pub app: String,
    pub run: String,
    pub attempt: u32,
    pub method: String,
    #[serde(with = "b64")]
    pub args: Vec<u8>,
    /// Hex SHA-256 of the component.
    pub bundle: String,
    pub caller: Caller,
    /// The app's owner as it was when the run started.
    pub owner: String,
    /// The run's fixed clock.
    pub start_ts_ms: u64,
    pub budget: RunBudget,
    pub inputs: Vec<RunInput>,
    /// The noise seed, wrapped to the job node.
    #[serde(with = "b64")]
    pub seed: Vec<u8>,
    /// The run capability, hex.
    pub token: String,
    /// The cluster identity's signature over [`assignment_message`], hex.
    #[serde(default)]
    pub signature: String,
}

impl std::fmt::Debug for Assignment {
    /// Without the token and the wrapped keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assignment")
            .field("app", &self.app)
            .field("run", &self.run)
            .field("attempt", &self.attempt)
            .field("method", &self.method)
            .field("bundle", &self.bundle)
            .field("inputs", &self.inputs)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

fn push_prefixed(out: &mut Vec<u8>, field: &[u8]) -> EyreResult<()> {
    let len = u32::try_from(field.len()).map_err(|_| eyre!("a field is too long"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(field);
    Ok(())
}

fn push_caller(out: &mut Vec<u8>, caller: &Caller) -> EyreResult<()> {
    match caller {
        Caller::Account { account } => {
            out.push(1);
            push_prefixed(out, account.as_bytes())
        }
        Caller::ApiKey { key, account } => {
            out.push(2);
            push_prefixed(out, key.as_bytes())?;
            push_prefixed(out, account.as_bytes())
        }
        Caller::Public => {
            out.push(3);
            Ok(())
        }
        Caller::App { app } => {
            out.push(4);
            push_prefixed(out, app.as_bytes())
        }
        Caller::System => {
            out.push(5);
            Ok(())
        }
    }
}

/// The bytes the cluster identity signs: the label, then every field but the signature,
/// each length-prefixed, integers big-endian.
pub fn assignment_message(a: &Assignment) -> EyreResult<Vec<u8>> {
    let mut m = ASSIGNMENT_LABEL.to_vec();
    push_prefixed(&mut m, a.app.as_bytes())?;
    push_prefixed(&mut m, a.run.as_bytes())?;
    m.extend_from_slice(&a.attempt.to_be_bytes());
    push_prefixed(&mut m, a.method.as_bytes())?;
    push_prefixed(&mut m, &a.args)?;
    push_prefixed(&mut m, a.bundle.as_bytes())?;
    push_caller(&mut m, &a.caller)?;
    push_prefixed(&mut m, a.owner.as_bytes())?;
    m.extend_from_slice(&a.start_ts_ms.to_be_bytes());
    m.extend_from_slice(&a.budget.fuel.to_be_bytes());
    m.extend_from_slice(&a.budget.memory_mib.to_be_bytes());
    m.extend_from_slice(&a.budget.wall_secs.to_be_bytes());
    let count = u32::try_from(a.inputs.len()).map_err(|_| eyre!("too many inputs"))?;
    m.extend_from_slice(&count.to_be_bytes());
    for input in &a.inputs {
        push_prefixed(&mut m, input.id.as_bytes())?;
        m.extend_from_slice(&input.size.to_be_bytes());
        push_prefixed(&mut m, input.sha256.as_bytes())?;
        push_prefixed(&mut m, &input.wrapped)?;
    }
    push_prefixed(&mut m, &a.seed)?;
    push_prefixed(&mut m, a.token.as_bytes())?;
    Ok(m)
}

/// SHA-256 of [`assignment_message`]: what a report is signed for.
pub fn assignment_digest(a: &Assignment) -> EyreResult<[u8; 32]> {
    Ok(calimero_identity::sha256(&assignment_message(a)?))
}

/// The signature (hex) of `assignment` under the cluster identity key.
pub fn sign_assignment(key: &SigningKey, assignment: &Assignment) -> EyreResult<String> {
    let signature = key.sign(&assignment_message(assignment)?);
    Ok(hex::encode(signature.to_bytes()))
}

/// Whether `assignment` is signed by the cluster identity `identity`.
#[must_use]
pub fn verify_assignment(identity: &[u8; 32], assignment: &Assignment) -> bool {
    let Ok(message) = assignment_message(assignment) else {
        return false;
    };
    verify(identity, &message, &assignment.signature)
}

fn bundle_message(hash: &str) -> Vec<u8> {
    [BUNDLE_LABEL, hash.as_bytes()].concat()
}

/// The signature (hex) that lets a job node take the component `hash`.
pub fn sign_bundle(key: &SigningKey, hash: &str) -> EyreResult<String> {
    if !is_hash(hash) {
        bail!("a component is named by its hex SHA-256");
    }
    Ok(hex::encode(key.sign(&bundle_message(hash)).to_bytes()))
}

/// Whether `signature` (hex) is the cluster identity's over the component `hash`.
#[must_use]
pub fn verify_bundle(identity: &[u8; 32], hash: &str, signature: &str) -> bool {
    verify(identity, &bundle_message(hash), signature)
}

fn verify(key: &[u8; 32], message: &[u8], signature_hex: &str) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(key) else {
        return false;
    };
    let Some(signature) = hex::decode(signature_hex)
        .ok()
        .and_then(|b| Signature::from_slice(&b).ok())
    else {
        return false;
    };
    key.verify_strict(message, &signature).is_ok()
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportStatus {
    Done,
    Failed,
}

/// Why a run failed: the app's own doing, never the node's (a node-local failure is a lost
/// attempt and is not reported).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailReason {
    Fuel,
    Memory,
    Time,
    Trap,
    AppError,
    Integrity,
    Cancelled,
}

impl FailReason {
    /// The name signed and served; the same as the serde name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fuel => "fuel",
            Self::Memory => "memory",
            Self::Time => "time",
            Self::Trap => "trap",
            Self::AppError => "app-error",
            Self::Integrity => "integrity",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A file a run wrote: the hash of its ciphertext, and its plaintext size and hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputRef {
    pub id: String,
    pub size: u64,
    pub sha256: String,
}

/// What a run cost, in the integers that are signed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportUsage {
    pub cpu_micros: u64,
    /// Peak memory times wall time, in GiB-microseconds.
    pub gib_micros: u64,
    pub fuel: u64,
}

impl ReportUsage {
    /// From thread CPU, the peak bytes the engine granted and the wall time of the run.
    #[must_use]
    pub fn measured(
        cpu: std::time::Duration,
        peak_memory: u64,
        wall: std::time::Duration,
        fuel: u64,
    ) -> Self {
        let gib_micros = u128::from(peak_memory) * wall.as_micros() / BYTES_PER_GIB;
        Self {
            cpu_micros: u64::try_from(cpu.as_micros()).unwrap_or(u64::MAX),
            gib_micros: u64::try_from(gib_micros).unwrap_or(u64::MAX),
            fuel,
        }
    }

    /// GiB-seconds, for a meter.
    #[must_use]
    pub fn gib_seconds(&self) -> f64 {
        self.gib_micros as f64 / MICROS_PER_SECOND as f64
    }
}

/// The final report of one attempt, signed by its job node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub app: String,
    pub run: String,
    pub attempt: u32,
    pub status: ReportStatus,
    /// Some exactly when the status is `Failed`.
    pub reason: Option<FailReason>,
    /// The method's return bytes; empty unless the status is `Done`.
    #[serde(with = "b64")]
    pub result: Vec<u8>,
    pub outputs: Vec<OutputRef>,
    pub usage: ReportUsage,
}

/// The bytes a job node signs for `report` of the assignment with `assignment` as its digest.
pub fn report_message(report: &Report, assignment: &[u8; 32]) -> EyreResult<Vec<u8>> {
    match (report.status, report.reason) {
        (ReportStatus::Done, None) => {}
        (ReportStatus::Failed, Some(_))
            if report.result.is_empty() && report.outputs.is_empty() => {}
        _ => {
            bail!("a done report has no reason, and a failed one a reason and no result or outputs")
        }
    }
    let mut m = REPORT_LABEL.to_vec();
    m.extend_from_slice(assignment);
    push_prefixed(&mut m, report.app.as_bytes())?;
    push_prefixed(&mut m, report.run.as_bytes())?;
    m.extend_from_slice(&report.attempt.to_be_bytes());
    m.push(match report.status {
        ReportStatus::Done => 0,
        ReportStatus::Failed => 1,
    });
    push_prefixed(
        &mut m,
        report.reason.map_or("", FailReason::as_str).as_bytes(),
    )?;
    m.extend_from_slice(&calimero_identity::sha256(&report.result));
    let count = u32::try_from(report.outputs.len()).map_err(|_| eyre!("too many outputs"))?;
    m.extend_from_slice(&count.to_be_bytes());
    for output in &report.outputs {
        m.extend_from_slice(&hash_bytes(&output.id)?);
        m.extend_from_slice(&output.size.to_be_bytes());
        m.extend_from_slice(&hash_bytes(&output.sha256)?);
    }
    m.extend_from_slice(&report.usage.cpu_micros.to_be_bytes());
    m.extend_from_slice(&report.usage.gib_micros.to_be_bytes());
    m.extend_from_slice(&report.usage.fuel.to_be_bytes());
    Ok(m)
}

/// The signature (hex) of `report` under a job node's key.
pub fn sign_report(key: &SigningKey, assignment: &[u8; 32], report: &Report) -> EyreResult<String> {
    Ok(hex::encode(
        key.sign(&report_message(report, assignment)?).to_bytes(),
    ))
}

/// Whether `signature` (hex) is `job_key`'s over `report` for the assignment with that digest.
#[must_use]
pub fn verify_report(
    job_key: &[u8; 32],
    assignment: &[u8; 32],
    report: &Report,
    signature: &str,
) -> bool {
    let Ok(message) = report_message(report, assignment) else {
        return false;
    };
    verify(job_key, &message, signature)
}

#[cfg(test)]
pub(crate) fn sample_assignment() -> Assignment {
    Assignment {
        app: "app".into(),
        run: "run".into(),
        attempt: 1,
        method: "digest".into(),
        args: b"{}".to_vec(),
        bundle: "ab".repeat(32),
        caller: Caller::System,
        owner: "owner".into(),
        start_ts_ms: 1_700_000_000_000,
        budget: RunBudget {
            fuel: 10,
            memory_mib: 64,
            wall_secs: 5,
        },
        inputs: vec![RunInput {
            id: "cd".repeat(32),
            size: 7,
            sha256: "ef".repeat(32),
            wrapped: vec![1, 2, 3],
        }],
        seed: vec![4, 5, 6],
        token: "01".repeat(32),
        signature: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Edit<T> = (&'static str, Box<dyn Fn(&mut T)>);

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn public(seed: u8) -> [u8; 32] {
        key(seed).verifying_key().to_bytes()
    }

    fn report() -> Report {
        Report {
            app: "app".into(),
            run: "run".into(),
            attempt: 1,
            status: ReportStatus::Done,
            reason: None,
            result: b"[\"x\"]".to_vec(),
            outputs: vec![OutputRef {
                id: "aa".repeat(32),
                size: 9,
                sha256: "bb".repeat(32),
            }],
            usage: ReportUsage {
                cpu_micros: 11,
                gib_micros: 12,
                fuel: 13,
            },
        }
    }

    const FOR: [u8; 32] = [5; 32];

    fn failed_report(reason: FailReason) -> Report {
        Report {
            status: ReportStatus::Failed,
            reason: Some(reason),
            result: Vec::new(),
            outputs: Vec::new(),
            ..report()
        }
    }

    #[test]
    fn a_signed_assignment_verifies_under_its_cluster_identity_only() {
        let mut a = sample_assignment();
        a.signature = sign_assignment(&key(1), &a).unwrap();
        assert!(verify_assignment(&public(1), &a));
        assert!(!verify_assignment(&public(2), &a));
    }

    #[test]
    fn changing_any_field_of_a_signed_assignment_breaks_the_signature() {
        let mut signed = sample_assignment();
        signed.signature = sign_assignment(&key(1), &signed).unwrap();
        let edits: Vec<Edit<Assignment>> = vec![
            ("app", Box::new(|a| a.app.push('x'))),
            ("run", Box::new(|a| a.run.push('x'))),
            ("attempt", Box::new(|a| a.attempt += 1)),
            ("method", Box::new(|a| a.method.push('x'))),
            ("args", Box::new(|a| a.args.push(0))),
            ("bundle", Box::new(|a| a.bundle = "00".repeat(32))),
            ("caller kind", Box::new(|a| a.caller = Caller::Public)),
            (
                "caller account",
                Box::new(|a| {
                    a.caller = Caller::Account {
                        account: "x".into(),
                    }
                }),
            ),
            ("owner", Box::new(|a| a.owner.push('x'))),
            ("startTsMs", Box::new(|a| a.start_ts_ms += 1)),
            ("fuel", Box::new(|a| a.budget.fuel += 1)),
            ("memoryMib", Box::new(|a| a.budget.memory_mib += 1)),
            ("wallSecs", Box::new(|a| a.budget.wall_secs += 1)),
            ("input id", Box::new(|a| a.inputs[0].id.push('x'))),
            ("input size", Box::new(|a| a.inputs[0].size += 1)),
            (
                "input sha256",
                Box::new(|a| a.inputs[0].sha256 = "00".repeat(32)),
            ),
            ("input wrapped", Box::new(|a| a.inputs[0].wrapped.push(0))),
            ("no inputs", Box::new(|a| a.inputs.clear())),
            ("seed", Box::new(|a| a.seed.push(0))),
            ("token", Box::new(|a| a.token = "02".repeat(32))),
        ];
        for (name, edit) in edits {
            let mut a = signed.clone();
            edit(&mut a);
            assert!(!verify_assignment(&public(1), &a), "{name}");
        }
        assert!(verify_assignment(&public(1), &signed));
    }

    #[test]
    fn the_assignment_message_is_the_documented_byte_layout() {
        let mut a = sample_assignment();
        a.app = "a".into();
        a.run = "r".into();
        a.method = "m".into();
        a.args = vec![9];
        a.bundle = "b".into();
        a.caller = Caller::Account {
            account: "c".into(),
        };
        a.owner = "o".into();
        a.inputs = vec![RunInput {
            id: "i".into(),
            size: 5,
            sha256: "s".into(),
            wrapped: vec![7, 7],
        }];
        a.seed = vec![8];
        a.token = "t".into();
        a.attempt = 2;
        a.start_ts_ms = 3;
        a.budget = RunBudget {
            fuel: 4,
            memory_mib: 5,
            wall_secs: 6,
        };
        let mut want = b"calimero.compute.assignment.v1\0".to_vec();
        want.extend([0, 0, 0, 1, b'a', 0, 0, 0, 1, b'r', 0, 0, 0, 2]);
        want.extend([0, 0, 0, 1, b'm', 0, 0, 0, 1, 9, 0, 0, 0, 1, b'b']);
        want.extend([1, 0, 0, 0, 1, b'c', 0, 0, 0, 1, b'o']);
        want.extend(3u64.to_be_bytes());
        want.extend(4u64.to_be_bytes());
        want.extend([0, 0, 0, 5, 0, 0, 0, 6, 0, 0, 0, 1]);
        want.extend([0, 0, 0, 1, b'i']);
        want.extend(5u64.to_be_bytes());
        want.extend([0, 0, 0, 1, b's', 0, 0, 0, 2, 7, 7]);
        want.extend([0, 0, 0, 1, 8, 0, 0, 0, 1, b't']);
        assert_eq!(assignment_message(&a).unwrap(), want);
    }

    #[test]
    fn a_big_integer_signs_exactly() {
        let mut a = sample_assignment();
        a.start_ts_ms = (1 << 53) + 1;
        let one = assignment_message(&a).unwrap();
        a.start_ts_ms = 1 << 53;
        assert_ne!(one, assignment_message(&a).unwrap());
    }

    #[test]
    fn a_signature_that_is_not_hex_or_not_64_bytes_does_not_verify() {
        let mut a = sample_assignment();
        for bad in ["", "zz", "00", &"00".repeat(64)] {
            a.signature = bad.into();
            assert!(!verify_assignment(&public(1), &a), "{bad}");
        }
    }

    #[test]
    fn an_assignment_with_a_field_nobody_signed_is_refused() {
        let mut wire = serde_json::to_value(sample_assignment()).unwrap();
        let _prev = wire
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), 1.into());
        assert!(serde_json::from_value::<Assignment>(wire).is_err());
    }

    #[test]
    fn a_component_is_signed_by_name_and_only_the_cluster_identity_opens_it() {
        let hash = "ab".repeat(32);
        let signature = sign_bundle(&key(1), &hash).unwrap();
        assert!(verify_bundle(&public(1), &hash, &signature));
        assert!(!verify_bundle(&public(2), &hash, &signature));
        assert!(!verify_bundle(&public(1), &"cd".repeat(32), &signature));
        assert!(!verify_bundle(&public(1), &hash, ""));
        // An assignment's signature is not a component's.
        let mut a = sample_assignment();
        a.signature = sign_assignment(&key(1), &a).unwrap();
        assert!(!verify_bundle(&public(1), &hash, &a.signature));
    }

    #[test]
    fn debug_shows_neither_the_token_nor_the_wrapped_keys() {
        let mut a = sample_assignment();
        a.token = "feedface".into();
        a.inputs[0].wrapped = vec![0xAB, 0xCD];
        let shown = format!("{a:?}");
        assert!(
            !shown.contains("feedface") && !shown.contains("171"),
            "{shown}"
        );
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn a_signed_report_verifies_under_its_job_key_for_its_assignment_only() {
        let signature = sign_report(&key(3), &FOR, &report()).unwrap();
        assert!(verify_report(&public(3), &FOR, &report(), &signature));
        assert!(!verify_report(&public(4), &FOR, &report(), &signature));
        assert!(!verify_report(&public(3), &[6; 32], &report(), &signature));
    }

    #[test]
    fn changing_any_field_of_a_signed_report_breaks_the_signature() {
        let signature = sign_report(&key(3), &FOR, &report()).unwrap();
        let edits: Vec<Edit<Report>> = vec![
            ("app", Box::new(|r| r.app.push('x'))),
            ("run", Box::new(|r| r.run.push('x'))),
            ("attempt", Box::new(|r| r.attempt += 1)),
            ("status", Box::new(|r| r.status = ReportStatus::Failed)),
            ("reason", Box::new(|r| r.reason = Some(FailReason::Fuel))),
            ("result", Box::new(|r| r.result.push(0))),
            ("output id", Box::new(|r| r.outputs[0].id.push('x'))),
            ("output size", Box::new(|r| r.outputs[0].size += 1)),
            (
                "output sha256",
                Box::new(|r| r.outputs[0].sha256 = "00".repeat(32)),
            ),
            ("no outputs", Box::new(|r| r.outputs.clear())),
            (
                "another output",
                Box::new(|r| r.outputs.push(r.outputs[0].clone())),
            ),
            ("cpu", Box::new(|r| r.usage.cpu_micros += 1)),
            ("gib", Box::new(|r| r.usage.gib_micros += 1)),
            ("fuel", Box::new(|r| r.usage.fuel += 1)),
        ];
        for (name, edit) in edits {
            let mut r = report();
            edit(&mut r);
            assert!(!verify_report(&public(3), &FOR, &r, &signature), "{name}");
        }
        assert!(verify_report(&public(3), &FOR, &report(), &signature));
    }

    #[test]
    fn every_failure_reason_signs_differently_and_names_its_serde_form() {
        let reasons = [
            FailReason::Fuel,
            FailReason::Memory,
            FailReason::Time,
            FailReason::Trap,
            FailReason::AppError,
            FailReason::Integrity,
            FailReason::Cancelled,
        ];
        let mut messages = Vec::new();
        for reason in reasons {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
            let r = failed_report(reason);
            messages.push(report_message(&r, &FOR).unwrap());
        }
        messages.sort();
        messages.dedup();
        assert_eq!(messages.len(), reasons.len());
    }

    #[test]
    fn a_component_name_must_be_a_hash_to_be_signed() {
        for bad in ["", "ab", &"AB".repeat(32), &"zz".repeat(32)] {
            assert!(sign_bundle(&key(1), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_output_hash_that_is_not_a_sha256_cannot_be_signed_or_verified() {
        let mut r = report();
        r.outputs[0].sha256 = "bb".into();
        assert!(sign_report(&key(3), &FOR, &r).is_err());
        assert!(!verify_report(&public(3), &FOR, &r, &"00".repeat(64)));
    }

    #[test]
    fn the_report_message_is_the_documented_byte_layout() {
        let r = Report {
            app: "a".into(),
            run: "r".into(),
            attempt: 2,
            status: ReportStatus::Failed,
            reason: Some(FailReason::Fuel),
            result: Vec::new(),
            outputs: Vec::new(),
            usage: ReportUsage {
                cpu_micros: 1,
                gib_micros: 2,
                fuel: 3,
            },
        };
        let mut want = b"calimero.compute.report.v1\0".to_vec();
        want.extend(FOR);
        want.extend([0, 0, 0, 1, b'a', 0, 0, 0, 1, b'r', 0, 0, 0, 2, 1]);
        want.extend([0, 0, 0, 4]);
        want.extend(b"fuel");
        want.extend(calimero_identity::sha256(b""));
        want.extend([0, 0, 0, 0]);
        want.extend([
            0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 3,
        ]);
        assert_eq!(report_message(&r, &FOR).unwrap(), want);

        // A finished run's outputs sign their ids as the 32 bytes they spell.
        let mut done = report();
        done.outputs[0].id = "0a".repeat(32);
        done.outputs[0].sha256 = "07".repeat(32);
        let message = report_message(&done, &FOR).unwrap();
        let tail = &message[message.len() - 24 - 32 - 8 - 32..];
        assert_eq!(&tail[..32], [0x0a; 32]);
        assert_eq!(&tail[32..40], 9u64.to_be_bytes());
        assert_eq!(&tail[40..72], [7; 32]);
    }

    #[test]
    fn a_report_has_one_encoding_and_anything_else_is_refused() {
        let signature = sign_report(&key(3), &FOR, &report()).unwrap();
        assert!(verify_report(&public(3), &FOR, &report(), &signature));
        let refused: Vec<Edit<Report>> = vec![
            (
                "uppercase id",
                Box::new(|r| r.outputs[0].id = "AA".repeat(32)),
            ),
            (
                "uppercase hash",
                Box::new(|r| r.outputs[0].sha256 = "BB".repeat(32)),
            ),
            ("short id", Box::new(|r| r.outputs[0].id = "aa".into())),
            (
                "a done run with a reason",
                Box::new(|r| r.reason = Some(FailReason::Trap)),
            ),
            (
                "a failed run without one",
                Box::new(|r| {
                    r.status = ReportStatus::Failed;
                    r.result.clear();
                    r.outputs.clear();
                }),
            ),
        ];
        for (name, edit) in refused {
            let mut r = report();
            edit(&mut r);
            assert!(report_message(&r, &FOR).is_err(), "{name}");
            assert!(sign_report(&key(3), &FOR, &r).is_err(), "{name}");
            assert!(!verify_report(&public(3), &FOR, &r, &signature), "{name}");
        }
        // A failed run reports neither a result nor partial outputs.
        let mut with_result = failed_report(FailReason::Fuel);
        with_result.result = b"x".to_vec();
        let mut with_outputs = failed_report(FailReason::Fuel);
        with_outputs.outputs = report().outputs;
        for r in [with_result, with_outputs] {
            assert!(report_message(&r, &FOR).is_err());
        }
        assert!(report_message(&failed_report(FailReason::Fuel), &FOR).is_ok());
    }

    #[test]
    fn a_signature_that_only_non_strict_verification_accepts_is_refused() {
        use ed25519_dalek::Verifier;
        // R is the neutral point and s is 0: valid under the neutral key without any secret.
        let mut neutral = [0u8; 32];
        neutral[0] = 1;
        let mut raw = [0u8; 64];
        raw[0] = 1;
        let mut a = sample_assignment();
        a.signature = hex::encode(raw);
        let message = assignment_message(&a).unwrap();
        let weak = VerifyingKey::from_bytes(&neutral).unwrap();
        assert!(weak.verify(&message, &Signature::from_bytes(&raw)).is_ok());
        assert!(!verify_assignment(&neutral, &a));
    }

    #[test]
    fn gib_seconds_are_peak_bytes_times_wall() {
        let usage = ReportUsage::measured(
            std::time::Duration::from_millis(1500),
            2 << 30,
            std::time::Duration::from_secs(3),
            7,
        );
        assert_eq!(usage.cpu_micros, 1_500_000);
        assert_eq!(usage.gib_micros, 6_000_000);
        assert!((usage.gib_seconds() - 6.0).abs() < f64::EPSILON);
        assert_eq!(usage.fuel, 7);
    }
}
