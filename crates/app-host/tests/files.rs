//! The host core's `files` policy, limits and outcome.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use calimero_app_host::{
    Caller, Directory, FileInfo, FileRead, FileWrite, Files, HostCore, HostDenied, Invocation,
    InvocationKind, MemoryFiles, NewFile, OrgRole, Outcome, ReaderId, Status, TimerCtx, TrapKind,
    Usage, WriterId, FILES_DISABLED, FILE_CREATED_IN_RUN, FILE_DELETED_TWICE, FILE_FAILED,
    FILE_FINISHED, FILE_TOO_LARGE, FILE_TOO_MANY_DELETES, FILE_TOO_MANY_OPEN, MAX_DELETES,
    MAX_FILE_BYTES, MAX_OPEN_FILES, MAX_READ_CHUNK, MAX_RUN_FILE_BYTES, NO_SUCH_FILE,
    RUN_WROTE_TOO_MUCH,
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
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0 += bytes.len() as u64;
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, String> {
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
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, String> {
        Err(NO_SUCH_FILE.to_owned())
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, String> {
        Ok(Box::new(CountingWriter(0)))
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
        h.files_delete(&id).unwrap().unwrap();
    }
    let mut h = host(timer_inv(), Arc::new(files.clone()));
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"x").unwrap().unwrap();
    h.writer_finish(w).unwrap().unwrap();
    h.files_delete(&id).unwrap().unwrap();
    for kind in no_writes {
        let mut h = host(inv(kind), Arc::new(files.clone()));
        denied(h.files_create());
        denied(h.files_delete(&id));
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
    files.apply(&out.new_files, &out.deleted_files);

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
    assert!(h.writer_finish(w).unwrap().is_ok());
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
fn delete_is_recorded_not_applied_and_only_for_known_files() {
    let files = MemoryFiles::default();
    let id = files.insert(b"data");
    let mut h = call_host(&files);
    assert_eq!(h.files_delete("nope").unwrap().unwrap_err(), NO_SUCH_FILE);
    h.files_delete(&id).unwrap().unwrap();
    assert!(files.get(&id).is_some());
    let out = finish(h, Status::Ok);
    assert_eq!(out.deleted_files, vec![id.clone()]);
    files.apply(&out.new_files, &out.deleted_files);
    assert!(files.get(&id).is_none());
}

#[test]
fn deleting_twice_or_a_file_created_in_the_run_is_a_soft_error() {
    let files = MemoryFiles::default();
    let id = files.insert(b"data");
    let mut h = call_host(&files);
    h.files_delete(&id).unwrap().unwrap();
    assert_eq!(
        h.files_delete(&id).unwrap().unwrap_err(),
        FILE_DELETED_TWICE
    );
    let w = h.files_create().unwrap().unwrap();
    h.writer_write(w, b"fresh").unwrap().unwrap();
    let fresh = h.writer_finish(w).unwrap().unwrap();
    assert_eq!(
        h.files_delete(&fresh).unwrap().unwrap_err(),
        FILE_CREATED_IN_RUN
    );
    let out = finish(h, Status::Ok);
    assert_eq!(out.deleted_files, vec![id]);
}

#[test]
fn deletes_are_capped_per_run() {
    let files = MemoryFiles::default();
    let ids: Vec<String> = (0..=MAX_DELETES)
        .map(|n| files.insert(n.to_string().as_bytes()))
        .collect();
    let mut h = call_host(&files);
    for id in &ids[..MAX_DELETES] {
        h.files_delete(id).unwrap().unwrap();
    }
    assert_eq!(
        h.files_delete(&ids[MAX_DELETES]).unwrap().unwrap_err(),
        FILE_TOO_MANY_DELETES
    );
}

#[test]
fn a_failing_run_drops_new_and_deleted_files() {
    let files = MemoryFiles::default();
    let id = files.insert(b"data");
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
        h.files_delete(&id).unwrap().unwrap();
        let out = finish(h, status);
        assert!(out.new_files.is_empty());
        assert!(out.deleted_files.is_empty());
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
    assert_eq!(h.files_delete("x").unwrap().unwrap_err(), NO_SUCH_FILE);
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
    for _ in 0..8 {
        assert_eq!(
            h.writer_write(first, &chunk).unwrap().unwrap_err(),
            FILE_TOO_LARGE
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

#[test]
fn a_deleted_file_is_gone_for_the_rest_of_the_run() {
    let files = MemoryFiles::default();
    let id = files.insert(b"data");
    let mut h = call_host(&files);
    h.files_delete(&id).unwrap().unwrap();
    assert!(h.files_info(&id).unwrap().is_none());
    assert_eq!(h.files_open(&id).unwrap().unwrap_err(), NO_SUCH_FILE);
    assert_eq!(
        h.files_delete(&id).unwrap().unwrap_err(),
        FILE_DELETED_TWICE
    );
}

/// Returns the whole rest of the file whatever `max` says.
struct OverReturning(Vec<u8>);

impl FileRead for OverReturning {
    fn read(&mut self, _max: usize) -> Result<Vec<u8>, String> {
        Ok(std::mem::take(&mut self.0))
    }
}

struct OverReturningFiles(Vec<u8>);

impl Files for OverReturningFiles {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, String> {
        Ok(Box::new(OverReturning(self.0.clone())))
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, String> {
        Err(FILES_DISABLED.to_owned())
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
    fn write(&mut self, _bytes: &[u8]) -> Result<(), String> {
        if self.0 {
            return Err(PROVIDER_ERROR.to_owned());
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<NewFile, String> {
        Err(PROVIDER_ERROR.to_owned())
    }
}

impl Files for Failing {
    fn open(&self, _id: &str) -> Result<Box<dyn FileRead>, String> {
        Err(NO_SUCH_FILE.to_owned())
    }
    fn info(&self, _id: &str) -> Option<FileInfo> {
        None
    }
    fn create(&self) -> Result<Box<dyn FileWrite>, String> {
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
