//! One run on its own OS thread: the compute invocation over the run's files, and the signed
//! report of how it ended.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use calimero_app_host::{
    Budget, ComputeCaps, ComputeCtx, Directory, FileError, FileInfo, FileRead, FileWrite, Files,
    HostCore, Invocation, InvocationKind, NewFile, OrgRole, Outcome, Progress, ReadError, Status,
    TrapKind, NO_SUCH_FILE,
};
use ring::digest::{Context, SHA256};
use tracing::warn;
use zeroize::Zeroizing;

use super::runs::{hash_hex, Finished, Keys, OutputFile, Phase, Run};
use super::{locked, JobNode};
use crate::files::{sealed_len, FileOpener, FileSealer, CHUNK_BYTES, TAG_BYTES};
use crate::job_wire::{
    sign_report, Assignment, FailReason, OutputRef, Report, ReportStatus, ReportUsage, RunInput,
};
use crate::usage::{self, Run as Measured};

const INPUT_PREFIX: &str = "in-"; // an input's ciphertext: in-{index}
const OUTPUT_PREFIX: &str = "out-"; // a finished output: out-{index}
const PART_SUFFIX: &str = ".part"; // a file still being written
const THREAD_NAME: &str = "compute-run"; // name of a run's OS thread
const FINISHED: &str = "the file is finished"; // soft error of a write after finish
const CHANGED_ON_DISK: &str = "the input changed on disk"; // read error: the node's own disk altered an input

/// How a run ended, as far as its leader is concerned.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Done,
    Failed(FailReason),
    /// This node could not serve the run: no outcome exists, and the leader runs it elsewhere.
    Lost,
}

/// Maps an outcome to what is reported, by what happened and never by its text. An input that
/// does not open during a run is the node's disk, never the leader's doing: the run is lost.
pub(super) fn classify(status: &Status) -> Verdict {
    match status {
        Status::Ok => Verdict::Done,
        Status::AppError { .. } => Verdict::Failed(FailReason::AppError),
        Status::Trap { kind, .. } => match kind {
            TrapKind::OutOfFuel => Verdict::Failed(FailReason::Fuel),
            TrapKind::OutOfMemory => Verdict::Failed(FailReason::Memory),
            TrapKind::Timeout => Verdict::Failed(FailReason::Time),
            TrapKind::Cancelled => Verdict::Failed(FailReason::Cancelled),
            TrapKind::Unavailable => Verdict::Lost,
            TrapKind::StackOverflow | TrapKind::Fault | TrapKind::HostDenied => {
                Verdict::Failed(FailReason::Trap)
            }
        },
    }
}

/// What a compute run knows of its surroundings: nothing but its app's owner.
struct ComputeDirectory {
    owner: String,
}

impl Directory for ComputeDirectory {
    fn org(&self) -> Option<String> {
        None
    }
    fn app_owner(&self) -> String {
        self.owner.clone()
    }
    fn org_role(&self, _account: &str) -> Option<OrgRole> {
        None
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }
    fn team_exists(&self, _team: &str) -> bool {
        false
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

/// A sealed file in the scratch directory, an input or an output.
struct SealedFile {
    /// Hex SHA-256 of the ciphertext.
    id: String,
    size: u64,
    sha256: [u8; 32],
    key: Zeroizing<[u8; 32]>,
    path: PathBuf,
}

/// The files of one run: its inputs, opened by id and decrypted as they are read, and the
/// outputs, sealed to disk as they are written.
struct JobFiles {
    dir: PathBuf,
    inputs: Vec<SealedFile>,
    produced: Arc<Mutex<Vec<SealedFile>>>,
    writers: AtomicU64,
}

impl Files for JobFiles {
    fn open(&self, id: &str) -> Result<Box<dyn FileRead>, FileError> {
        let Some(input) = self.inputs.iter().find(|i| i.id == id) else {
            return Err(FileError::Failed(NO_SUCH_FILE.to_owned()));
        };
        let file = File::open(&input.path).map_err(|_| FileError::Unavailable)?;
        Ok(Box::new(InputReader {
            file,
            opener: Some(FileOpener::new(&input.key)),
            size: input.size,
            sha256: input.sha256,
            pending: Vec::new(),
            buf: vec![0; CHUNK_BYTES + TAG_BYTES],
        }))
    }

    fn info(&self, id: &str) -> Option<FileInfo> {
        let input = self.inputs.iter().find(|i| i.id == id)?;
        Some(FileInfo {
            size: input.size,
            sha256: input.sha256.to_vec(),
        })
    }

    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        let n = self.writers.fetch_add(1, Ordering::Relaxed);
        let part = self.dir.join(format!("w-{n}{PART_SUFFIX}"));
        let file = File::create(&part).map_err(|_| FileError::Unavailable)?;
        Ok(Box::new(OutputWriter {
            sealer: Some(FileSealer::new()),
            file: BufWriter::new(file),
            ciphertext: Context::new(&SHA256),
            part,
            dir: self.dir.clone(),
            produced: Arc::clone(&self.produced),
        }))
    }
}

struct InputReader {
    file: File,
    /// None once the end of the file was reached and proved authentic.
    opener: Option<FileOpener>,
    size: u64,
    sha256: [u8; 32],
    pending: Vec<u8>,
    buf: Vec<u8>,
}

impl FileRead for InputReader {
    fn read(&mut self, max: usize) -> Result<Vec<u8>, ReadError> {
        loop {
            if !self.pending.is_empty() {
                let take = max.min(self.pending.len());
                return Ok(self.pending.drain(..take).collect());
            }
            let Some(opener) = self.opener.as_mut() else {
                return Ok(Vec::new());
            };
            let n = self
                .file
                .read(&mut self.buf)
                .map_err(|e| ReadError::Failed(e.to_string()))?;
            let plain = if n == 0 {
                self.opener
                    .take()
                    .map(|o| o.finish_expecting(self.size, &self.sha256))
                    .unwrap_or_else(|| Ok(Vec::new()))
            } else {
                opener.push(&self.buf[..n])
            };
            // An input was proved whole when it arrived: failing now is the node's own disk.
            self.pending = plain.map_err(|_| ReadError::Failed(CHANGED_ON_DISK.to_owned()))?;
        }
    }
}

struct OutputWriter {
    sealer: Option<FileSealer>,
    file: BufWriter<File>,
    ciphertext: Context,
    part: PathBuf,
    dir: PathBuf,
    produced: Arc<Mutex<Vec<SealedFile>>>,
}

impl OutputWriter {
    fn put(&mut self, ciphertext: &[u8]) -> Result<(), String> {
        self.ciphertext.update(ciphertext);
        self.file.write_all(ciphertext).map_err(|e| e.to_string())
    }
}

impl FileWrite for OutputWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let sealer = self.sealer.as_mut().ok_or(FINISHED)?;
        let sealed = sealer.write(bytes).map_err(|e| e.to_string())?;
        self.put(&sealed)
    }

    fn finish(mut self: Box<Self>) -> Result<NewFile, String> {
        let sealer = self.sealer.take().ok_or(FINISHED)?;
        let key = Zeroizing::new(*sealer.data_key());
        let sealed = sealer.finish().map_err(|e| e.to_string())?;
        self.put(&sealed.tail)?;
        self.file.flush().map_err(|e| e.to_string())?;
        let mut id = [0u8; 32];
        id.copy_from_slice(self.ciphertext.clone().finish().as_ref());
        let mut produced = locked(&self.produced);
        let path = self.dir.join(format!("{OUTPUT_PREFIX}{}", produced.len()));
        std::fs::rename(&self.part, &path).map_err(|e| e.to_string())?;
        let id = hex::encode(id);
        produced.push(SealedFile {
            id: id.clone(),
            size: sealed.size,
            sha256: sealed.sha256,
            key,
            path,
        });
        // The ciphertext stays on disk; the leader fetches it from here.
        Ok(NewFile {
            id,
            size: sealed.size,
            sha256: sealed.sha256.to_vec(),
            wrapped_key: Vec::new(),
            ciphertext: Vec::new(),
        })
    }
}

/// Writes the ciphertext of input `i` into the run's directory.
pub(super) async fn store_input(dir: &Path, i: usize, bytes: &[u8]) -> std::io::Result<()> {
    let part = dir.join(format!("{INPUT_PREFIX}{i}{PART_SUFFIX}"));
    tokio::fs::write(&part, bytes).await?;
    tokio::fs::rename(&part, dir.join(format!("{INPUT_PREFIX}{i}"))).await
}

/// Starts `run` on its own thread, once, when every input is in.
pub(super) fn start(job: &Arc<JobNode>, run: &Arc<Run>) {
    {
        let mut state = locked(&run.state);
        if !matches!(state.phase, Phase::Awaiting) {
            return;
        }
        state.phase = Phase::Running;
    }
    let (job_for_thread, run_for_thread) = (Arc::clone(job), Arc::clone(run));
    let spawned = std::thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || thread_main(&job_for_thread, &run_for_thread));
    if let Err(e) = spawned {
        warn!(error = %e, "cannot start a compute run's thread");
        run.end(Phase::Lost);
    }
}

/// Ends a run as lost if its thread leaves without ending it, a panic included.
struct EndGuard<'a>(&'a Run);

impl Drop for EndGuard<'_> {
    fn drop(&mut self) {
        if matches!(locked(&self.0.state).phase, Phase::Running) {
            self.0.end(Phase::Lost);
        }
    }
}

fn thread_main(job: &JobNode, run: &Arc<Run>) {
    let _guard = EndGuard(run);
    let phase = execute(job, run).unwrap_or_else(|e| {
        warn!(error = %e, "a compute run could not be served");
        Phase::Lost
    });
    let keep_outputs = matches!(&phase, Phase::Done(f) if f.report.status == ReportStatus::Done);
    remove_files(&run.dir, keep_outputs);
    run.end(phase);
}

fn input_files(
    dir: &Path,
    inputs: &[RunInput],
    keys: Vec<Zeroizing<[u8; 32]>>,
) -> eyre::Result<Vec<SealedFile>> {
    inputs
        .iter()
        .zip(keys)
        .enumerate()
        .map(|(i, (input, key))| {
            Ok(SealedFile {
                id: input.id.clone(),
                size: input.size,
                sha256: hash_hex(&input.sha256).ok_or_else(|| eyre::eyre!("a bad input hash"))?,
                key,
                path: dir.join(format!("{INPUT_PREFIX}{i}")),
            })
        })
        .collect()
}

fn execute(job: &JobNode, run: &Arc<Run>) -> eyre::Result<Phase> {
    let a = &run.assignment;
    let Keys { seed, inputs } = locked(&run.keys)
        .take()
        .ok_or_else(|| eyre::eyre!("the run's keys were already taken"))?;
    let produced = Arc::new(Mutex::new(Vec::new()));
    let files = JobFiles {
        dir: run.dir.clone(),
        inputs: input_files(&run.dir, &a.inputs, inputs)?,
        produced: Arc::clone(&produced),
        writers: AtomicU64::new(0),
    };
    let invocation = Invocation {
        kind: InvocationKind::Action,
        method: a.method.clone(),
        args: a.args.clone(),
        caller: a.caller.to_host(),
        entry_ts_ms: a.start_ts_ms,
        id_term: 0,
        id_seq: 0,
        seed: *seed,
        timer: None,
        compute: Some(ComputeCtx {
            run_id: a.run.clone(),
            owner: a.owner.clone(),
            inputs: a.inputs.iter().map(|i| i.id.clone()).collect(),
        }),
    };
    let for_progress = Arc::clone(run);
    let progress: Progress = Arc::new(move |percent, step| {
        locked(&for_progress.state).progress = Some((percent, step.to_owned()));
    });
    let host = HostCore::new(
        &a.app,
        Arc::new(BTreeMap::<Vec<u8>, Vec<u8>>::new()),
        0,
        invocation,
        Arc::new(ComputeDirectory {
            owner: a.owner.clone(),
        }),
    )
    .map_err(|e| eyre::eyre!("{e}"))?
    .with_files(Arc::new(files))
    .with_progress(progress);
    let budget = Budget::compute(&ComputeCaps {
        fuel: a.budget.fuel,
        memory_mib: a.budget.memory_mib,
        wall_secs: a.budget.wall_secs,
        result: String::new(),
    });
    let (outcome, measured) = usage::measure(|| {
        job.engine
            .invoke_cancellable(&run.prepared, host, budget, Arc::clone(&run.cancel))
    });
    let produced = std::mem::take(&mut *locked(&produced));
    ending(job, &run.digest(), a, outcome, measured, produced)
}

/// The signed ending of a run, or nothing to sign when the node could not serve it.
fn ending(
    job: &JobNode,
    assignment: &[u8; 32],
    a: &Assignment,
    outcome: Outcome,
    measured: Measured,
    produced: Vec<SealedFile>,
) -> eyre::Result<Phase> {
    let (status, reason) = match classify(&outcome.status) {
        Verdict::Lost => return Ok(Phase::Lost),
        Verdict::Done => (ReportStatus::Done, None),
        Verdict::Failed(reason) => (ReportStatus::Failed, Some(reason)),
    };
    let (result, kept) = match status {
        ReportStatus::Done => (outcome.result, produced),
        ReportStatus::Failed => (Vec::new(), Vec::new()),
    };
    let report = Report {
        app: a.app.clone(),
        run: a.run.clone(),
        attempt: a.attempt,
        status,
        reason,
        result,
        outputs: kept
            .iter()
            .map(|p| OutputRef {
                id: p.id.clone(),
                size: p.size,
                sha256: hex::encode(p.sha256),
            })
            .collect(),
        usage: ReportUsage::measured(
            measured.cpu,
            outcome.usage.peak_memory,
            measured.wall,
            outcome.usage.fuel,
        ),
    };
    let signature = sign_report(&job.job_key, assignment, &report)?;
    let bytes = kept.iter().map(|p| sealed_len(p.size)).sum();
    let outputs = kept
        .into_iter()
        .map(|p| OutputFile {
            path: p.path,
            key: p.key,
        })
        .collect();
    Ok(Phase::Done(Box::new(Finished {
        report,
        signature,
        outputs,
        bytes,
    })))
}

/// Whether `ciphertext` opens under `key` to a file of `size` bytes hashing to `sha256`: what
/// the leader declared of an input, which is checked once, when the input arrives.
pub(super) fn input_matches(
    ciphertext: &[u8],
    key: &[u8; 32],
    size: u64,
    sha256: &[u8; 32],
) -> bool {
    let mut opener = FileOpener::new(key);
    for piece in ciphertext.chunks(CHUNK_BYTES + TAG_BYTES) {
        if opener.push(piece).is_err() {
            return false;
        }
    }
    opener.finish_expecting(size, sha256).is_ok()
}

/// Ends a run that has not started because an input is not what its mark declares: the leader
/// sent it, so the report is signed.
pub(super) fn fail_integrity(job: &JobNode, run: &Run) {
    let a = &run.assignment;
    let report = Report {
        app: a.app.clone(),
        run: a.run.clone(),
        attempt: a.attempt,
        status: ReportStatus::Failed,
        reason: Some(FailReason::Integrity),
        result: Vec::new(),
        outputs: Vec::new(),
        usage: ReportUsage::default(),
    };
    let phase = match sign_report(&job.job_key, &run.digest(), &report) {
        Ok(signature) => Phase::Done(Box::new(Finished {
            report,
            signature,
            outputs: Vec::new(),
            bytes: 0,
        })),
        Err(e) => {
            warn!(error = %e, "cannot sign a report");
            Phase::Lost
        }
    };
    if run.end_awaiting(phase) {
        *locked(&run.keys) = None;
        remove_files(&run.dir, false);
    }
}

/// Deletes what a run leaves on disk: everything, or everything but its finished outputs.
fn remove_files(dir: &Path, keep_outputs: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let keep = keep_outputs
            && entry
                .file_name()
                .to_string_lossy()
                .starts_with(OUTPUT_PREFIX)
            && !entry.file_name().to_string_lossy().ends_with(PART_SUFFIX);
        if !keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use calimero_app_host::Abi;
    use calimero_app_host::Prepared;

    use super::*;
    use crate::job_wire::sample_assignment;

    fn plain(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// An input of `len` bytes sealed into `dir/in-0`, and the files that serve it.
    fn files(dir: &Path, len: usize) -> (JobFiles, Vec<u8>, Vec<u8>) {
        let data = plain(len);
        let mut sealer = FileSealer::new();
        let mut ciphertext = sealer.write(&data).unwrap();
        let key = Zeroizing::new(*sealer.data_key());
        let sealed = sealer.finish().unwrap();
        ciphertext.extend_from_slice(&sealed.tail);
        std::fs::write(dir.join("in-0"), &ciphertext).unwrap();
        let files = JobFiles {
            dir: dir.to_owned(),
            inputs: vec![SealedFile {
                id: hex::encode(calimero_identity::sha256(&ciphertext)),
                size: sealed.size,
                sha256: sealed.sha256,
                key,
                path: dir.join("in-0"),
            }],
            produced: Arc::default(),
            writers: AtomicU64::new(0),
        };
        (files, data, ciphertext)
    }

    fn id(files: &JobFiles) -> String {
        files.inputs[0].id.clone()
    }

    fn read_all(mut reader: Box<dyn FileRead>, piece: usize) -> Result<Vec<u8>, ReadError> {
        let mut out = Vec::new();
        loop {
            let chunk = reader.read(piece)?;
            if chunk.is_empty() {
                return Ok(out);
            }
            assert!(chunk.len() <= piece);
            out.extend(chunk);
        }
    }

    #[test]
    fn every_trap_kind_has_one_verdict_and_none_is_integrity() {
        let trap = |kind| Status::Trap {
            kind,
            detail: "words that mean nothing here".into(),
        };
        let cases = [
            (TrapKind::OutOfFuel, Verdict::Failed(FailReason::Fuel)),
            (TrapKind::OutOfMemory, Verdict::Failed(FailReason::Memory)),
            (TrapKind::Timeout, Verdict::Failed(FailReason::Time)),
            (TrapKind::Cancelled, Verdict::Failed(FailReason::Cancelled)),
            (TrapKind::Unavailable, Verdict::Lost),
            (TrapKind::StackOverflow, Verdict::Failed(FailReason::Trap)),
            (TrapKind::Fault, Verdict::Failed(FailReason::Trap)),
            (TrapKind::HostDenied, Verdict::Failed(FailReason::Trap)),
        ];
        for (kind, want) in cases {
            assert_eq!(classify(&trap(kind)), want, "{kind:?}");
        }
        assert_eq!(classify(&Status::Ok), Verdict::Done);
        let app_error = Status::AppError {
            code: "c".into(),
            message: "m".into(),
        };
        assert_eq!(classify(&app_error), Verdict::Failed(FailReason::AppError));
    }

    #[test]
    fn a_thread_that_leaves_a_running_run_ends_it_lost_and_leaves_an_ended_one_alone() {
        let prepared = Arc::new(Prepared {
            hash: String::new(),
            abi: Abi::default(),
            inner: Arc::new(()),
        });
        let keys = || Keys {
            seed: Zeroizing::new([0; 32]),
            inputs: Vec::new(),
        };
        let new = || {
            Run::new(
                sample_assignment(),
                Vec::new(),
                [0; 32],
                Arc::clone(&prepared),
                keys(),
                PathBuf::new(),
            )
        };
        let run = new();
        locked(&run.state).phase = Phase::Running;
        drop(EndGuard(&run));
        assert!(matches!(locked(&run.state).phase, Phase::Lost));
        assert!(*run.finished.borrow());

        let ended = new();
        locked(&ended.state).phase = Phase::Running;
        ended.end(Phase::Lost);
        let at = locked(&ended.state).ended;
        drop(EndGuard(&ended));
        assert_eq!(locked(&ended.state).ended, at);
    }

    #[test]
    fn an_input_reads_back_in_any_piece_size_and_reports_what_it_is() {
        let dir = tempfile::tempdir().unwrap();
        for len in [0, 1, CHUNK_BYTES, CHUNK_BYTES + 1, 3 * CHUNK_BYTES + 5] {
            let (files, data, _) = files(dir.path(), len);
            for piece in [1000, CHUNK_BYTES, 1 << 20] {
                let read = read_all(files.open(&id(&files)).unwrap(), piece).unwrap();
                assert_eq!(read, data, "{len} in {piece}");
            }
            let info = files.info(&id(&files)).unwrap();
            assert_eq!(info.size, len as u64);
        }
        let (files, _, _) = files(dir.path(), 1);
        assert!(files.info("other").is_none());
    }

    #[test]
    fn a_file_that_is_not_an_input_does_not_open() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _, _) = files(dir.path(), 10);
        assert_eq!(
            files.open("other").err(),
            Some(FileError::Failed(NO_SUCH_FILE.to_owned()))
        );
    }

    #[test]
    fn an_input_matches_only_the_file_it_was_declared_to_be() {
        let dir = tempfile::tempdir().unwrap();
        let len = 3 * CHUNK_BYTES + 5;
        let (files, data, ciphertext) = files(dir.path(), len);
        let input = &files.inputs[0];
        let key = &*input.key;
        let (size, sha256) = (input.size, input.sha256);
        assert!(input_matches(&ciphertext, key, size, &sha256));
        assert!(!input_matches(&ciphertext, key, size, &[9; 32]));
        assert!(!input_matches(&ciphertext, key, size + 1, &sha256));
        assert!(!input_matches(&ciphertext, &[1; 32], size, &sha256));
        let mut flipped = ciphertext.clone();
        flipped[ciphertext.len() / 2] ^= 1;
        assert!(!input_matches(&flipped, key, size, &sha256));
        assert!(!input_matches(
            &ciphertext[..ciphertext.len() - 1],
            key,
            size,
            &sha256
        ));
        assert!(!input_matches(
            &ciphertext[..CHUNK_BYTES],
            key,
            size,
            &sha256
        ));
        // Authentic bytes of a file other than the one the mark names.
        assert!(!input_matches(
            &ciphertext,
            key,
            size,
            &calimero_identity::sha256(&data[1..])
        ));
    }

    #[test]
    fn an_input_that_changes_on_disk_during_a_run_is_never_the_leaders_doing() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _, ciphertext) = files(dir.path(), 3 * CHUNK_BYTES + 5);
        let path = dir.path().join("in-0");
        let mut flipped = ciphertext.clone();
        flipped[ciphertext.len() / 2] ^= 1;
        std::fs::write(&path, flipped).unwrap();
        let e = read_all(files.open(&id(&files)).unwrap(), 1 << 20).unwrap_err();
        assert!(matches!(e, ReadError::Failed(_)), "{e:?}");
        // Put back as it was, the failure it caused is still the node's trouble.
        std::fs::write(&path, &ciphertext).unwrap();
        std::fs::write(&path, &ciphertext[..ciphertext.len() - CHUNK_BYTES]).unwrap();
        let e = read_all(files.open(&id(&files)).unwrap(), 1 << 20).unwrap_err();
        assert!(matches!(e, ReadError::Failed(_)), "{e:?}");
        std::fs::write(&path, &ciphertext).unwrap();
        assert!(read_all(files.open(&id(&files)).unwrap(), 1 << 20).is_ok());
    }

    #[test]
    fn an_input_stored_after_the_run_directory_is_gone_reports_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let e = rt.block_on(store_input(&gone, 0, b"x")).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn a_disk_that_fails_is_the_nodes_trouble_and_not_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _, _) = files(dir.path(), 10);
        std::fs::remove_file(dir.path().join("in-0")).unwrap();
        assert_eq!(files.open(&id(&files)).err(), Some(FileError::Unavailable));
        std::fs::remove_dir_all(dir.path()).unwrap();
        assert_eq!(files.create().err(), Some(FileError::Unavailable));
    }

    #[test]
    fn outputs_are_sealed_on_disk_each_under_a_key_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _, _) = files(dir.path(), 0);
        let mut made = Vec::new();
        for n in 0..2usize {
            let data = plain(2 * CHUNK_BYTES + n);
            let mut writer = files.create().unwrap();
            for piece in data.chunks(10_000) {
                writer.write(piece).unwrap();
            }
            made.push((writer.finish().unwrap(), data));
        }
        let produced = locked(&files.produced);
        assert_eq!(produced.len(), 2);
        assert_ne!(*produced[0].key, *produced[1].key);
        for (i, (new, data)) in made.iter().enumerate() {
            let stored = std::fs::read(dir.path().join(format!("out-{i}"))).unwrap();
            assert_eq!(new.id, hex::encode(calimero_identity::sha256(&stored)));
            assert_eq!(
                (new.size, new.ciphertext.len(), new.wrapped_key.len()),
                (data.len() as u64, 0, 0)
            );
            assert_eq!(produced[i].id, new.id);
            let mut opener = FileOpener::new(&produced[i].key);
            let mut opened = opener.push(&stored).unwrap();
            opened.extend(
                opener
                    .finish_expecting(new.size, &produced[i].sha256)
                    .unwrap(),
            );
            assert_eq!(&opened, data);
            for at in [0, CHUNK_BYTES, 2 * CHUNK_BYTES - 64] {
                let window = &data[at..at + 64];
                assert!(
                    !stored.windows(64).any(|s| s == window),
                    "plaintext at {at}"
                );
            }
        }
    }

    #[test]
    fn a_writer_that_is_never_finished_leaves_no_output_and_its_part_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (files, _, _) = files(dir.path(), 4);
        let mut writer = files.create().unwrap();
        writer.write(&plain(100)).unwrap();
        drop(writer);
        assert!(files.produced.lock().unwrap().is_empty());
        let finished = files.create().unwrap().finish().unwrap();
        assert_eq!(finished.size, 0);

        remove_files(dir.path(), true);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["out-0"]);
        remove_files(dir.path(), false);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
