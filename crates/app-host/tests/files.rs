//! The host core's `files` policy, limits and outcome.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use calimero_app_host::{
    parse_blob_id, BlobHost, BlobInfo, Caller, Directory, FileError, FileInfo, FileRead, FileWrite,
    Files, HostCore, HostDenied, Invocation, InvocationKind, MemoryFiles, NewFile, OrgRole,
    Outcome, ReadError, ReaderId, Status, TimerCtx, TrapKind, Usage, WriterId, FILES_DISABLED,
    FILE_CORRUPT, FILE_FAILED, FILE_FINISHED, FILE_TOO_LARGE, FILE_TOO_MANY_NEW,
    FILE_TOO_MANY_OPEN, FILE_UNAVAILABLE, MAX_FILE_BYTES, MAX_NEW_FILES, MAX_OPEN_FILES,
    MAX_READ_CHUNK, MAX_RUN_FILE_BYTES, NO_SUCH_FILE, RUN_WROTE_TOO_MUCH,
};

struct NoDirectory;

impl Directory for NoDirectory {
    fn org(&self) -> Option<String> {
        None
    }
    fn app_owner(&self) -> String {
        "owner".to_owned()
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

/// A provider whose writers count bytes and keep none, so the size limits can be hit.
struct CountingFiles;

struct CountingWriter(u64);

impl FileWrite for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        self.0 += bytes.len() as u64;
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, FileError> {
        Ok(NewFile {
            id: "counted".to_owned(),
            size: self.0,
            sha256: Vec::new(),
            wrapped_key: Vec::new(),
            ciphertext: Vec::new(),
        })
    }
}

impl Files for CountingFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Err(FileError::Failed(NO_SUCH_FILE.to_owned()))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Ok(Box::new(CountingWriter(0)))
    }
}

/// A provider whose one file reads two bytes and then fails as the given error.
struct FailingFiles(ReadError);

struct FailingReader(Option<ReadError>, bool);

impl FileRead for FailingReader {
    fn read(&mut self, _max: usize) -> Result<Vec<u8>, ReadError> {
        if !self.1 {
            self.1 = true;
            return Ok(b"ab".to_vec());
        }
        Err(self.0.take().unwrap())
    }
}

impl Files for FailingFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Ok(Box::new(FailingReader(Some(self.0.clone()), false)))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Err(FileError::Failed(FILES_DISABLED.to_owned()))
    }
}

fn inv(kind: InvocationKind) -> Invocation {
    Invocation {
        kind,
        method: "m".to_owned(),
        args: Vec::new(),
        caller: Caller::Account("ada".to_owned()),
        entry_ts_ms: 1_000,
        id_term: 3,
        id_seq: 42,
        seed: [7; 32],
        timer: None,
    }
}

fn timer_inv() -> Invocation {
    Invocation {
        timer: Some(TimerCtx {
            scheduled_ms: 1,
            previous_ms: None,
        }),
        ..inv(InvocationKind::Timer)
    }
}

fn host(inv: Invocation, files: Arc<dyn Files>) -> HostCore {
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    HostCore::new("app", Arc::new(state), 0, inv, Arc::new(NoDirectory))
        .unwrap()
        .with_files(files)
}

fn call_host(files: &MemoryFiles) -> HostCore {
    host(inv(InvocationKind::Call), Arc::new(files.clone()))
}

fn finish(host: HostCore, status: Status) -> Outcome {
    host.finish(status, Vec::new(), Usage::default())
}

fn denied<T: Debug>(result: Result<T, HostDenied>) {
    result.expect_err("expected HostDenied");
}

#[test]
fn writes_are_logged_kinds_only_reads_are_everything_but_migrate() {
    let files = MemoryFiles::default();
    let id = files.insert(b"hello");
    let writes = [
        InvocationKind::Call,
        InvocationKind::Init,
        InvocationKind::XcallDeliver,
    ];
    let no_writes = [
        InvocationKind::View,
        InvocationKind::Action,
        InvocationKind::Migrate,
    ];
    let reads = [
        InvocationKind::Call,
        InvocationKind::View,
        InvocationKind::Action,
        InvocationKind::Init,
        InvocationKind::XcallDeliver,
    ];
    for kind in writes {
        let mut h = host(inv(kind), Arc::new(files.clone()));
        let w = h.files_create().unwrap().unwrap();
        h.writer_write(w, b"x").unwrap().unwrap();
        h.writer_finish(w).unwrap().unwrap();
    }
    let mut h = host(timer_inv(), Arc::new(files.clone()));
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"x").unwrap().unwrap();
    h.writer_finish(w).unwrap().unwrap();
    for kind in no_writes {
        let mut h = host(inv(kind), Arc::new(files.clone()));
        denied(h.files_create());
    }
    for kind in reads {
        let mut h = host(inv(kind), Arc::new(files.clone()));
        assert!(h.files_info(&id).unwrap().is_some());
        let r = h.files_open(&id).unwrap().unwrap();
        assert_eq!(h.reader_read(r, 10).unwrap().unwrap(), b"hello");
    }
    let mut h = host(inv(InvocationKind::Migrate), Arc::new(files.clone()));
    denied(h.files_info(&id));
    denied(h.files_open(&id));
}

#[test]
fn write_finish_then_read_back_in_chunks_to_the_end() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"hello ").unwrap().unwrap();
    h.writer_write(w, b"world").unwrap().unwrap();
    let id = h.writer_finish(w).unwrap().unwrap();
    h.writer_close(w);
    let out = finish(h, Status::Ok);
    files.apply(&out.new_files);

    let mut h = host(inv(InvocationKind::View), Arc::new(files));
    let info = h.files_info(&id).unwrap().unwrap();
    assert_eq!(info.size, 11);
    let r = h.files_open(&id).unwrap().unwrap();
    assert_eq!(h.reader_read(r, 4).unwrap().unwrap(), b"hell");
    assert_eq!(h.reader_read(r, 100).unwrap().unwrap(), b"o world");
    assert!(h.reader_read(r, 4).unwrap().unwrap().is_empty());
}

#[test]
fn open_and_info_of_an_unknown_file() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    assert!(h.files_info("nope").unwrap().is_none());
    assert_eq!(h.files_open("nope").unwrap().unwrap_err(), NO_SUCH_FILE);
}

#[test]
fn an_unknown_handle_is_denied() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    denied(h.writer_write(WriterId(9), b"x"));
    denied(h.writer_finish(WriterId(9)));
    denied(h.reader_read(ReaderId(9), 1));
}

#[test]
fn read_chunks_are_clamped() {
    let files = MemoryFiles::default();
    let big = vec![1u8; MAX_READ_CHUNK as usize + 10];
    let id = files.insert(&big);
    let mut h = call_host(&files);
    let r = h.files_open(&id).unwrap().unwrap();
    assert_eq!(
        h.reader_read(r, u32::MAX).unwrap().unwrap().len(),
        MAX_READ_CHUNK as usize
    );
}

#[test]
fn open_handles_are_capped_and_closing_frees_a_slot() {
    let files = MemoryFiles::default();
    let id = files.insert(b"x");
    let mut h = call_host(&files);
    let mut readers = Vec::new();
    for _ in 0..MAX_OPEN_FILES / 2 {
        readers.push(h.files_open(&id).unwrap().unwrap());
        h.files_create().unwrap().unwrap();
    }
    assert_eq!(h.files_open(&id).unwrap().unwrap_err(), FILE_TOO_MANY_OPEN);
    assert_eq!(h.files_create().unwrap().unwrap_err(), FILE_TOO_MANY_OPEN);
    h.reader_close(readers[0]);
    assert!(h.files_create().unwrap().is_ok());
}

#[test]
fn a_file_over_the_per_file_cap_is_a_soft_error_and_writing_stops_there() {
    let mut h = host(inv(InvocationKind::Call), Arc::new(CountingFiles));
    let w = h.files_create().unwrap().unwrap();
    let chunk = vec![0u8; 1024 * 1024];
    let mib = MAX_FILE_BYTES / (1024 * 1024);
    for _ in 0..mib {
        h.writer_write(w, &chunk).unwrap().unwrap();
    }
    h.writer_write(w, &chunk[..(MAX_FILE_BYTES % (1024 * 1024)) as usize])
        .unwrap()
        .unwrap();
    assert_eq!(
        h.writer_write(w, b"x").unwrap().unwrap_err(),
        FILE_TOO_LARGE
    );
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), FILE_FAILED);
    assert_eq!(h.writer_write(w, b"x").unwrap().unwrap_err(), FILE_FAILED);
}

#[test]
fn a_run_over_the_run_cap_is_a_soft_error() {
    let mut h = host(inv(InvocationKind::Call), Arc::new(CountingFiles));
    let chunk = vec![0u8; 1024 * 1024];
    let per_file = MAX_FILE_BYTES / (1024 * 1024);
    let mut written = 0;
    let mut result = Ok(());
    while result.is_ok() {
        let w = h.files_create().unwrap().unwrap();
        for _ in 0..per_file {
            result = h.writer_write(w, &chunk).unwrap();
            if result.is_err() {
                break;
            }
            written += chunk.len() as u64;
        }
        h.writer_close(w);
    }
    assert_eq!(result.unwrap_err(), RUN_WROTE_TOO_MUCH);
    assert!(written <= MAX_RUN_FILE_BYTES);
    assert!(written + chunk.len() as u64 > MAX_RUN_FILE_BYTES);
}

#[test]
fn a_write_refused_for_the_run_cap_fails_the_writer_so_finish_seals_nothing() {
    let mut h = host(inv(InvocationKind::Call), Arc::new(CountingFiles));
    let chunk = vec![0u8; 1024 * 1024];
    let mut last = None;
    while last.is_none() {
        let w = h.files_create().unwrap().unwrap();
        for _ in 0..MAX_FILE_BYTES / (1024 * 1024) {
            if let Err(reason) = h.writer_write(w, &chunk).unwrap() {
                assert_eq!(reason, RUN_WROTE_TOO_MUCH);
                last = Some(w);
                break;
            }
        }
        if last.is_none() {
            h.writer_close(w);
        }
    }
    let w = last.unwrap();
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), FILE_FAILED);
    let out = finish(h, Status::Ok);
    assert!(out.new_files.is_empty());
}

#[test]
fn finishing_twice_and_writing_after_finish_are_soft_errors() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    let w = h.files_create().unwrap().unwrap();
    h.writer_finish(w).unwrap().unwrap();
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), FILE_FINISHED);
    assert_eq!(h.writer_write(w, b"x").unwrap().unwrap_err(), FILE_FINISHED);
    let out = finish(h, Status::Ok);
    assert_eq!(out.new_files.len(), 1);
}

#[test]
fn a_dropped_writer_leaves_no_file() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"abandoned").unwrap().unwrap();
    h.writer_close(w);
    let out = finish(h, Status::Ok);
    assert!(out.new_files.is_empty());
}

#[test]
fn a_failing_run_drops_its_new_files() {
    let files = MemoryFiles::default();
    let statuses = [
        Status::AppError {
            code: "x".to_owned(),
            message: String::new(),
        },
        Status::Trap {
            kind: TrapKind::Fault,
            detail: String::new(),
        },
    ];
    for status in statuses {
        let mut h = call_host(&files);
        let w = h.files_create().unwrap().unwrap();
        h.writer_finish(w).unwrap().unwrap();
        let out = finish(h, status);
        assert!(out.new_files.is_empty());
    }
}

#[test]
fn the_outcome_carries_the_new_file_with_its_key_material() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"abc").unwrap().unwrap();
    let id = h.writer_finish(w).unwrap().unwrap();
    let out = finish(h, Status::Ok);
    let file = &out.new_files[0];
    assert_eq!((file.id.as_str(), file.size), (id.as_str(), 3));
    assert_eq!(file.sha256.len(), 32);
}

#[test]
fn no_files_refuses_everything() {
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut h = HostCore::new(
        "app",
        Arc::new(state),
        0,
        inv(InvocationKind::Call),
        Arc::new(NoDirectory),
    )
    .unwrap();
    assert_eq!(h.files_create().unwrap().unwrap_err(), FILES_DISABLED);
    assert_eq!(h.files_open("x").unwrap().unwrap_err(), NO_SUCH_FILE);
    assert!(h.files_info("x").unwrap().is_none());
}

#[test]
fn closing_a_writer_frees_a_slot() {
    let files = MemoryFiles::default();
    let mut h = call_host(&files);
    let writers: Vec<WriterId> = (0..MAX_OPEN_FILES)
        .map(|_| h.files_create().unwrap().unwrap())
        .collect();
    assert_eq!(h.files_create().unwrap().unwrap_err(), FILE_TOO_MANY_OPEN);
    h.writer_close(writers[0]);
    assert!(h.files_create().unwrap().is_ok());
}

#[test]
fn a_refused_oversized_write_does_not_count_toward_the_run_cap() {
    let mut h = host(inv(InvocationKind::Call), Arc::new(CountingFiles));
    let chunk = vec![0u8; 1024 * 1024];
    let first = h.files_create().unwrap().unwrap();
    for _ in 0..MAX_FILE_BYTES / (1024 * 1024) {
        h.writer_write(first, &chunk).unwrap().unwrap();
    }
    let rest = (MAX_FILE_BYTES % (1024 * 1024)) as usize;
    h.writer_write(first, &chunk[..rest]).unwrap().unwrap();
    assert_eq!(
        h.writer_write(first, &chunk).unwrap().unwrap_err(),
        FILE_TOO_LARGE
    );
    for _ in 0..8 {
        assert_eq!(
            h.writer_write(first, &chunk).unwrap().unwrap_err(),
            FILE_FAILED
        );
    }
    let second = h.files_create().unwrap().unwrap();
    let room = MAX_RUN_FILE_BYTES - MAX_FILE_BYTES;
    for _ in 0..room / (1024 * 1024) {
        h.writer_write(second, &chunk).unwrap().unwrap();
    }
    assert_eq!(
        h.writer_write(second, &chunk).unwrap().unwrap_err(),
        RUN_WROTE_TOO_MUCH
    );
}

/// Returns the whole rest of the file whatever `max` says.
struct OverReturning(Vec<u8>);

impl FileRead for OverReturning {
    fn read(&mut self, _max: usize) -> Result<Vec<u8>, ReadError> {
        Ok(std::mem::take(&mut self.0))
    }
}

struct OverReturningFiles(Vec<u8>);

impl Files for OverReturningFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Ok(Box::new(OverReturning(self.0.clone())))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Err(FileError::Failed(FILES_DISABLED.to_owned()))
    }
}

#[test]
fn a_provider_that_returns_more_than_max_is_truncated_and_the_rest_kept() {
    let data: Vec<u8> = (0..10).collect();
    let mut h = host(
        inv(InvocationKind::Call),
        Arc::new(OverReturningFiles(data.clone())),
    );
    let r = h.files_open("f").unwrap().unwrap();
    assert_eq!(h.reader_read(r, 4).unwrap().unwrap(), data[..4]);
    assert_eq!(h.reader_read(r, 4).unwrap().unwrap(), data[4..8]);
    assert_eq!(h.reader_read(r, 4).unwrap().unwrap(), data[8..]);
    assert!(h.reader_read(r, 4).unwrap().unwrap().is_empty());
}

#[test]
fn an_over_returning_provider_is_clamped_to_the_chunk_cap() {
    let data = vec![1u8; MAX_READ_CHUNK as usize + 5];
    let mut h = host(
        inv(InvocationKind::Call),
        Arc::new(OverReturningFiles(data)),
    );
    let r = h.files_open("f").unwrap().unwrap();
    assert_eq!(
        h.reader_read(r, u32::MAX).unwrap().unwrap().len(),
        MAX_READ_CHUNK as usize
    );
    assert_eq!(h.reader_read(r, u32::MAX).unwrap().unwrap().len(), 5);
}

#[test]
fn a_read_of_zero_still_makes_progress() {
    let files = MemoryFiles::default();
    let id = files.insert(b"abc");
    let mut h = call_host(&files);
    let r = h.files_open(&id).unwrap().unwrap();
    assert_eq!(h.reader_read(r, 0).unwrap().unwrap(), b"a");
}

/// Fails the write or the finish with a fixed provider message.
struct Failing {
    on_write: bool,
}

const PROVIDER_ERROR: &str = "disk on fire";

struct FailingWriter(bool);

impl FileWrite for FailingWriter {
    fn write(&mut self, _bytes: &[u8]) -> Result<(), FileError> {
        if self.0 {
            return Err(FileError::Failed(PROVIDER_ERROR.to_owned()));
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, FileError> {
        Err(FileError::Failed(PROVIDER_ERROR.to_owned()))
    }
}

impl Files for Failing {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Err(FileError::Failed(NO_SUCH_FILE.to_owned()))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Ok(Box::new(FailingWriter(self.on_write)))
    }
}

#[test]
fn a_provider_failure_in_write_is_passed_through_then_the_writer_is_failed() {
    let mut h = host(
        inv(InvocationKind::Call),
        Arc::new(Failing { on_write: true }),
    );
    let w = h.files_create().unwrap().unwrap();
    assert_eq!(
        h.writer_write(w, b"x").unwrap().unwrap_err(),
        PROVIDER_ERROR
    );
    assert_eq!(h.writer_write(w, b"x").unwrap().unwrap_err(), FILE_FAILED);
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), FILE_FAILED);
    assert!(finish(h, Status::Ok).new_files.is_empty());
}

#[test]
fn a_provider_failure_in_finish_does_not_read_as_finished() {
    let mut h = host(
        inv(InvocationKind::Call),
        Arc::new(Failing { on_write: false }),
    );
    let w = h.files_create().unwrap().unwrap();
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), PROVIDER_ERROR);
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), FILE_FAILED);
    assert_eq!(h.writer_write(w, b"x").unwrap().unwrap_err(), FILE_FAILED);
    assert!(finish(h, Status::Ok).new_files.is_empty());
}

fn blob_ref(id: &str) -> Vec<u8> {
    let mut value = vec![0xD9, 0xCA, 0x1B, 0x58, 0x20];
    value.extend_from_slice(&parse_blob_id(id).unwrap());
    value
}

#[test]
fn blobs_info_answers_for_a_file_with_an_empty_mime() {
    let files = MemoryFiles::default();
    let held = files.insert(b"hello");
    let mut h = call_host(&files);
    let info = h.blobs_info(&held).unwrap().unwrap();
    assert_eq!((info.size, info.mime.as_str()), (5, ""));
    assert_eq!(info.sha256, h.files_info(&held).unwrap().unwrap().sha256);
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"fresh").unwrap().unwrap();
    let fresh = h.writer_finish(w).unwrap().unwrap();
    assert_eq!(h.blobs_info(&fresh).unwrap().unwrap().size, 5);
    assert_eq!(h.files_info(&fresh).unwrap().unwrap().size, 5);
    assert!(h.blobs_info(&"0".repeat(64)).unwrap().is_none());
}

/// A blob store that knows every id, so only the file rule can refuse a grant.
struct AnyBlob;

impl BlobHost for AnyBlob {
    fn info(&self, _id: &[u8; 32]) -> Option<BlobInfo> {
        Some(BlobInfo {
            size: 1,
            sha256: Vec::new(),
            mime: String::new(),
        })
    }

    fn grant(&self, _id: &[u8; 32], _expires_ms: u64) -> String {
        "grant".to_owned()
    }
}

#[test]
fn blobs_grant_refuses_a_sealed_file_but_not_a_blob() {
    let files = MemoryFiles::default();
    let held = files.insert(b"hello");
    let mut h = call_host(&files).with_blobs(Arc::new(AnyBlob));
    denied(h.blobs_grant(&held, 60));
    let w = h.files_create().unwrap().unwrap();
    let fresh = h.writer_finish(w).unwrap().unwrap();
    denied(h.blobs_grant(&fresh, 60));
    assert_eq!(h.blobs_grant(&"1".repeat(64), 60).unwrap(), "grant");
}

#[test]
fn a_blob_ref_to_a_file_may_be_stored_and_one_to_nothing_may_not() {
    let files = MemoryFiles::default();
    let held = files.insert(b"hello");
    let mut h = call_host(&files);
    h.storage_put(b"held", &blob_ref(&held)).unwrap();
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"fresh").unwrap().unwrap();
    let fresh = h.writer_finish(w).unwrap().unwrap();
    h.storage_put(b"fresh", &blob_ref(&fresh)).unwrap();
    denied(h.storage_put(b"none", &blob_ref(&"0".repeat(64))));
}

#[test]
fn a_corrupt_file_traps_the_run_and_other_read_failures_are_soft() {
    let mut h = host(
        inv(InvocationKind::View),
        Arc::new(FailingFiles(ReadError::Corrupt)),
    );
    let r = h.files_open("f").unwrap().unwrap();
    assert_eq!(h.reader_read(r, 10).unwrap().unwrap(), b"ab");
    let denied = h.reader_read(r, 10).unwrap_err();
    assert_eq!(denied, HostDenied(FILE_CORRUPT.to_owned()));

    let mut h = host(
        inv(InvocationKind::View),
        Arc::new(FailingFiles(ReadError::Failed("flaky".to_owned()))),
    );
    let r = h.files_open("f").unwrap().unwrap();
    assert_eq!(h.reader_read(r, 10).unwrap().unwrap(), b"ab");
    assert_eq!(h.reader_read(r, 10).unwrap().unwrap_err(), "flaky");
}

/// A provider that cannot get the bytes of any file now.
struct UnavailableFiles;

impl Files for UnavailableFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Err(FileError::Unavailable)
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Err(FileError::Unavailable)
    }
}

#[test]
fn an_unavailable_file_aborts_a_logged_run_as_a_timeout_and_is_soft_in_a_view() {
    for kind in [InvocationKind::Call, InvocationKind::Init] {
        let mut h = host(inv(kind), Arc::new(UnavailableFiles));
        denied(h.files_open("f"));
        denied(h.files_create());
        let trapped = Status::Trap {
            kind: TrapKind::HostDenied,
            detail: String::new(),
        };
        let out = finish(h, trapped);
        assert!(
            matches!(
                out.status,
                Status::Trap {
                    kind: TrapKind::Timeout,
                    ..
                }
            ),
            "{kind:?}: {:?}",
            out.status
        );
    }
    let mut h = host(inv(InvocationKind::View), Arc::new(UnavailableFiles));
    assert_eq!(h.files_open("f").unwrap().unwrap_err(), FILE_UNAVAILABLE);
    assert_eq!(finish(h, Status::Ok).status, Status::Ok);
}

#[test]
fn new_files_are_capped_per_run() {
    let files = CountingFiles;
    let mut h = host(inv(InvocationKind::Call), Arc::new(files));
    for _ in 0..MAX_NEW_FILES {
        let w = h.files_create().unwrap().unwrap();
        h.writer_finish(w).unwrap().unwrap();
        h.writer_close(w);
    }
    let w = h.files_create().unwrap().unwrap();
    assert_eq!(h.writer_finish(w).unwrap().unwrap_err(), FILE_TOO_MANY_NEW);
}

/// A provider whose writers cannot hold the bytes now: on a write, or on the finish.
struct FullFiles {
    on_write: bool,
}

struct FullWriter(bool);

impl FileWrite for FullWriter {
    fn write(&mut self, _bytes: &[u8]) -> Result<(), FileError> {
        if self.0 {
            return Err(FileError::Unavailable);
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, FileError> {
        Err(FileError::Unavailable)
    }
}

impl Files for FullFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, FileError> {
        Err(FileError::Failed(NO_SUCH_FILE.to_owned()))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, FileError> {
        Ok(Box::new(FullWriter(self.on_write)))
    }
}

#[test]
fn a_write_the_node_cannot_hold_aborts_the_logged_run_as_a_timeout() {
    for on_write in [true, false] {
        let mut h = host(inv(InvocationKind::Call), Arc::new(FullFiles { on_write }));
        let w = h.files_create().unwrap().unwrap();
        if on_write {
            denied(h.writer_write(w, b"x"));
        } else {
            h.writer_write(w, b"x").unwrap().unwrap();
            denied(h.writer_finish(w));
        }
        let trapped = Status::Trap {
            kind: TrapKind::HostDenied,
            detail: String::new(),
        };
        let out = finish(h, trapped);
        assert!(out.new_files.is_empty());
        assert!(
            matches!(
                out.status,
                Status::Trap {
                    kind: TrapKind::Timeout,
                    ..
                }
            ),
            "on_write {on_write}: {:?}",
            out.status
        );
    }
}
