//! Upload sessions: a large file arrives in parts of `PART_BYTES`, one part per segment, through a
//! session the app's leader keeps in memory. Each part is sealed into its segment, stored and
//! pushed to a majority; the quota is charged and the mark committed once, at `complete`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use calimero_app_host::MAX_FILE_BYTES;
use calimero_log::Group;
use reqwest::Method;
use serde_json::{json, Value};
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::files::{files_unavailable, segment_blob, uploaded, Stored, Upload, Uploaded};
use crate::app::AppMachine;
use crate::component::FileMark;
use crate::error::ApiError;
use crate::files::{file_id, wrap_key, SegmentWriter, PART_BYTES, SEGMENT_BYTES};
use crate::node::{off_workers, Node};
use crate::types::{AppRecord, Caller};

/// Sessions one app holds at once on its leader.
const MAX_UPLOADS_PER_APP: usize = 4;
/// Sessions one node holds at once.
const MAX_UPLOADS_PER_NODE: usize = 32;
/// Plaintext bytes the open sessions of one leader may declare together.
const MAX_UPLOAD_BYTES_IN_FLIGHT: u64 = 16 << 30;
/// A session with no part for this long ends.
const UPLOAD_IDLE: Duration = Duration::from_secs(15 * 60);
/// A session ends this long after it began, whatever it does.
const UPLOAD_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// The longest a part waits for its segment to reach a majority.
const PART_PUSH_WAIT: Duration = Duration::from_secs(20);
/// The longest a follower waits for the leader's answer to a forwarded upload request.
const PART_FORWARD_TIMEOUT: Duration = Duration::from_secs(25);
/// The idempotency key `complete` commits its mark under; a retried `complete` replays it.
const COMPLETE_KEY: &str = "upload-";

const _: () = assert!(
    PART_PUSH_WAIT.as_millis() < PART_FORWARD_TIMEOUT.as_millis()
        && PART_FORWARD_TIMEOUT.as_secs() < 30
);
// Session segments are not pinned: the sweep's grace must outlive any session.
const _: () = assert!(UPLOAD_TTL.as_secs() < crate::node::segments::SEGMENT_GRACE.as_secs());

/// What an upload request asks, as it travels from the node that got it to the leader.
pub(crate) enum UploadOp {
    Begin {
        size: u64,
    },
    Part {
        upload: String,
        k: usize,
        bytes: Bytes,
    },
    Status {
        upload: String,
    },
    Complete {
        upload: String,
    },
    Abort {
        upload: String,
    },
}

impl UploadOp {
    /// The peer request that carries this op to the leader.
    fn request(&self, app_id: &str, account: &str) -> (Method, String, Option<Bytes>) {
        let base = format!("internal/upload/{app_id}/{account}");
        match self {
            Self::Begin { size } => (
                Method::POST,
                format!("internal/upload-begin/{app_id}/{account}/{size}"),
                None,
            ),
            Self::Part { upload, k, bytes } => (
                Method::PUT,
                format!("{base}/{upload}/{k}"),
                Some(bytes.clone()),
            ),
            Self::Status { upload } => (Method::GET, format!("{base}/{upload}"), None),
            Self::Complete { upload } => (Method::POST, format!("{base}/{upload}/complete"), None),
            Self::Abort { upload } => (Method::DELETE, format!("{base}/{upload}"), None),
        }
    }
}

/// The sessions this node holds as an app leader.
#[derive(Default)]
pub(crate) struct Uploads(Mutex<HashMap<String, Arc<Session>>>);

struct Session {
    id: String,
    app_id: String,
    payer: String,
    account: String,
    size: u64,
    parts: usize,
    began_ms: u64,
    last_ms: AtomicU64,
    state: tokio::sync::Mutex<State>,
}

#[derive(Default)]
struct State {
    writer: Option<SegmentWriter>,
    /// Segment `k` is part `k`'s.
    segments: Vec<[u8; 32]>,
    /// SHA-256 of each part's plaintext, to tell a resent part from another.
    part_hashes: Vec<[u8; 32]>,
    /// Whether each segment is on a majority.
    pushed: Vec<bool>,
    /// Parts the client holds an answer for.
    acked: usize,
    finished: Option<Finished>,
    /// The quota was charged; a retried `complete` must not charge again.
    charged: bool,
    /// Ended: the session answers as unknown.
    gone: bool,
}

struct Finished {
    size: u64,
    sha256: [u8; 32],
    wrapped_key: Vec<u8>,
}

/// Why a session cannot be begun.
#[derive(Debug, PartialEq, Eq)]
enum Refused {
    App,
    Node,
    InFlight,
}

/// Whether a session of `size` bytes may begin on `app` beside the open ones `(app, size)`.
fn admit(open: &[(&str, u64)], app: &str, size: u64) -> Result<(), Refused> {
    if open.iter().filter(|(a, _)| *a == app).count() >= MAX_UPLOADS_PER_APP {
        return Err(Refused::App);
    }
    if open.len() >= MAX_UPLOADS_PER_NODE {
        return Err(Refused::Node);
    }
    let declared: u64 = open.iter().map(|(_, s)| s).sum();
    if declared.saturating_add(size) > MAX_UPLOAD_BYTES_IN_FLIGHT {
        return Err(Refused::InFlight);
    }
    Ok(())
}

/// Whether a session begun at `began_ms` and last given a part at `last_ms` has run out by `now_ms`.
fn lapsed(began_ms: u64, last_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_ms) >= UPLOAD_IDLE.as_millis() as u64
        || now_ms.saturating_sub(began_ms) >= UPLOAD_TTL.as_millis() as u64
}

fn parts_of(size: u64) -> usize {
    usize::try_from(size.div_ceil(PART_BYTES as u64).max(1)).unwrap_or(usize::MAX)
}

fn unknown() -> ApiError {
    ApiError::new(404, "UPLOAD_UNKNOWN", "no such upload: begin another")
}

fn refusal(status: u16, code: &str, message: &str, next: usize) -> ApiError {
    ApiError::new(status, code, message).with_details(json!({ "next": next }))
}

impl Session {
    fn expires_at(&self) -> u64 {
        let idle = self.last_ms.load(Ordering::Relaxed) + UPLOAD_IDLE.as_millis() as u64;
        idle.min(self.began_ms + UPLOAD_TTL.as_millis() as u64) / 1000
    }

    fn view(&self) -> Value {
        json!({
            "uploadId": self.id,
            "partBytes": PART_BYTES,
            "parts": self.parts,
            "expiresAt": self.expires_at(),
        })
    }

    /// The length part `k` must have: a whole part, but for the last.
    fn part_len(&self, k: usize) -> usize {
        if k + 1 < self.parts {
            PART_BYTES
        } else {
            usize::try_from(self.size - (k * PART_BYTES) as u64).unwrap_or(usize::MAX)
        }
    }
}

impl Node {
    /// Runs an upload request on the app's leader, here or one hop away.
    pub(crate) async fn upload_op(
        &self,
        app: &AppRecord,
        account: &str,
        op: UploadOp,
    ) -> Result<Value, ApiError> {
        let deadline = Instant::now() + PART_FORWARD_TIMEOUT;
        let mut last = ApiError::unavailable("no replica of the app answered");
        let mut leader: Option<String> = None;
        while Instant::now() < deadline {
            let done = if let Some(addr) = leader.take() {
                self.forward_upload_op(&addr, &app.app_id, account, &op)
                    .await
            } else if let Some(g) = self.replica_group(app) {
                match self.upload_op_local(&g, app, account, &op).await {
                    Err(Upload::Elsewhere(Some(addr))) => {
                        leader = Some(addr);
                        continue;
                    }
                    Err(Upload::Elsewhere(None)) => {
                        Err(ApiError::unavailable("the app has no leader yet"))
                    }
                    Ok(v) => return Ok(v),
                    Err(Upload::Refused(e) | Upload::Unknown(e)) => return Err(e),
                }
            } else if let Some(addr) = self.other_replica(app) {
                self.forward_upload_op(&addr, &app.app_id, account, &op)
                    .await
            } else {
                Err(last.clone())
            };
            match done {
                Err(e) if e.code == super::files::NOT_LEADER => {
                    leader = e.details["leaderUrl"].as_str().map(str::to_owned);
                    if leader.is_some() {
                        continue;
                    }
                    last = e;
                }
                Err(e) if e.code == "UNAVAILABLE" => last = e,
                done => return done,
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(last)
    }

    /// [`Self::upload_op`] as a peer asks it, one hop: a node that does not lead the app names
    /// the leader (`421`) instead of passing the request on.
    pub(crate) async fn upload_op_here(
        &self,
        app_id: &str,
        account: &str,
        op: UploadOp,
    ) -> Result<Value, ApiError> {
        let app = self
            .sys(|s| s.apps.get(app_id).cloned())
            .ok_or_else(|| ApiError::unavailable("no such app"))?;
        let g = self
            .hosted(app_id)
            .ok_or_else(|| ApiError::unavailable("app not hosted here"))?;
        match self.upload_op_local(&g, &app, account, &op).await {
            Ok(v) => Ok(v),
            Err(Upload::Refused(e) | Upload::Unknown(e)) => Err(e),
            Err(Upload::Elsewhere(Some(addr))) => Err(ApiError::new(
                421,
                super::files::NOT_LEADER,
                "send this to the app group's leader",
            )
            .with_details(json!({ "leaderUrl": addr }))),
            Err(Upload::Elsewhere(None)) => Err(ApiError::unavailable("the app has no leader yet")),
        }
    }

    async fn forward_upload_op(
        &self,
        addr: &str,
        app_id: &str,
        account: &str,
        op: &UploadOp,
    ) -> Result<Value, ApiError> {
        let (method, path, body) = op.request(app_id, account);
        let send = async {
            let resp = self.peer_raw(addr, method, &path, body).await?;
            eyre::Result::Ok(resp.json::<Value>().await?)
        };
        let sent: Result<eyre::Result<Value>, _> =
            tokio::time::timeout(PART_FORWARD_TIMEOUT, send).await;
        match sent {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e
                .downcast::<ApiError>()
                .unwrap_or_else(|e: eyre::Report| ApiError::unavailable(&e.to_string()))),
            Err(_) => Err(ApiError::unavailable(
                "the app's leader did not answer in time",
            )),
        }
    }

    /// The request on the app's leader; [`Upload::Elsewhere`] when this node is not it.
    async fn upload_op_local(
        &self,
        g: &Group<AppMachine>,
        app: &AppRecord,
        account: &str,
        op: &UploadOp,
    ) -> Result<Value, Upload> {
        let status = g.status();
        if !status.is_leader {
            self.drop_sessions_of(&app.app_id);
            let leader = status
                .leader
                .and_then(|id| self.sys(|s| s.nodes.get(&id).map(|n| n.addr.clone())));
            return Err(Upload::Elsewhere(leader));
        }
        let (app_id, now) = (app.app_id.as_str(), self.app_clock().now_ms());
        let done: Result<Value, ApiError> = async {
            match op {
                UploadOp::Begin { size } => self.begin_upload(app, account, *size, now),
                UploadOp::Complete { upload } => {
                    self.complete_upload(g, app, account, upload, now).await
                }
                UploadOp::Part { upload, k, bytes } => {
                    let session = self.session(upload, app_id, account, now)?;
                    session.last_ms.store(now, Ordering::Relaxed);
                    self.take_part(&session, *k, bytes).await
                }
                UploadOp::Status { upload } => {
                    let session = self.session(upload, app_id, account, now)?;
                    let next = session.state.lock().await.acked;
                    let mut view = session.view();
                    view["size"] = json!(session.size);
                    view["next"] = json!(next);
                    Ok(view)
                }
                UploadOp::Abort { upload } => {
                    let session = self.session(upload, app_id, account, now)?;
                    self.end_session(&session).await;
                    Ok(json!({}))
                }
            }
        }
        .await;
        done.map_err(Upload::Refused)
    }

    /// The session `id` of `account` on `app_id`, unless it ran out (then it ends).
    fn session(
        &self,
        id: &str,
        app_id: &str,
        account: &str,
        now: u64,
    ) -> Result<Arc<Session>, ApiError> {
        let found = (self.uploads.0.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .filter(|s| s.app_id == app_id && s.account == account)
            .cloned()
            .ok_or_else(unknown)?;
        if lapsed(found.began_ms, found.last_ms.load(Ordering::Relaxed), now) {
            self.end_later(&found);
            return Err(unknown());
        }
        Ok(found)
    }

    fn begin_upload(
        &self,
        app: &AppRecord,
        account: &str,
        size: u64,
        now: u64,
    ) -> Result<Value, ApiError> {
        if size > MAX_FILE_BYTES {
            return Err(crate::node::file_too_large());
        }
        let payer = crate::usage::payer(app);
        let (used, quota) = self.sys(|s| (s.file_bytes(&payer), s.quotas.max_file_bytes_per_org));
        let mut open = self
            .uploads
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let others: Vec<(&str, u64)> = open.values().map(|s| (s.app_id.as_str(), s.size)).collect();
        admit(&others, &app.app_id, size).map_err(|why| {
            let what = match why {
                Refused::App => "this app holds as many uploads as it may",
                Refused::Node => "this node holds as many uploads as it may",
                Refused::InFlight => "this node's open uploads declare as many bytes as it may",
            };
            ApiError::new(429, "TOO_MANY_UPLOADS", what)
        })?;
        let declared: u64 = (open.values())
            .filter(|s| s.payer == payer)
            .map(|s| s.size)
            .sum();
        if used.saturating_add(declared).saturating_add(size) > quota {
            return Err(ApiError::new(
                429,
                "QUOTA_EXCEEDED",
                "the file would pass the quota",
            ));
        }
        let session = Arc::new(Session {
            id: calimero_identity::b64url(&calimero_identity::random::<16>()),
            app_id: app.app_id.clone(),
            payer,
            account: account.to_owned(),
            size,
            parts: parts_of(size),
            began_ms: now,
            last_ms: AtomicU64::new(now),
            state: tokio::sync::Mutex::new(State {
                writer: Some(SegmentWriter::new()),
                ..State::default()
            }),
        });
        let view = session.view();
        let _none = open.insert(session.id.clone(), session);
        Ok(view)
    }

    /// Part `k`: sealed into its segment, stored here and pushed to a majority before the answer.
    async fn take_part(
        &self,
        session: &Session,
        k: usize,
        bytes: &Bytes,
    ) -> Result<Value, ApiError> {
        let mut st = session.state.lock().await;
        if st.gone {
            return Err(unknown());
        }
        let hash = off_workers(|| calimero_identity::sha256(bytes));
        if k < st.part_hashes.len() {
            if st.part_hashes[k] != hash {
                return Err(refusal(
                    409,
                    "PART_CONFLICT",
                    "part is not the one sent before",
                    st.acked,
                ));
            }
        } else if k == st.part_hashes.len() && st.acked == k && k < session.parts {
            if bytes.len() != session.part_len(k) {
                return Err(ApiError::bad_request(
                    "BAD_PART",
                    "part is not the length it must be",
                ));
            }
            self.seal_part(session, &mut st, bytes, hash)?;
        } else {
            return Err(refusal(
                409,
                "PART_OUT_OF_ORDER",
                "send the next part first",
                st.acked,
            ));
        }
        if !st.pushed[k] {
            let blob = segment_blob(&st.segments[k]);
            self.push_blobs(&session.app_id, &[blob], PART_PUSH_WAIT)
                .await?;
            st.pushed[k] = true;
        }
        st.acked = st.acked.max(k + 1);
        Ok(json!({ "next": k + 1 }))
    }

    /// Seals `bytes` as the next part and stores its segment here.
    fn seal_part(
        &self,
        session: &Session,
        st: &mut State,
        bytes: &Bytes,
        hash: [u8; 32],
    ) -> Result<(), ApiError> {
        let held = u32::try_from(PART_BYTES + SEGMENT_BYTES).map_err(|_| files_unavailable())?;
        let _budget = Arc::clone(&self.pending_file_bytes)
            .try_acquire_many_owned(held)
            .map_err(|_| files_unavailable())?;
        let last = st.part_hashes.len() + 1 == session.parts;
        let sealed = off_workers(|| self.cut_part(session, st, bytes, last))
            .map_err(|e| ApiError::internal(&e.to_string()))?;
        st.segments.push(sealed);
        st.part_hashes.push(hash);
        st.pushed.push(false);
        Ok(())
    }

    /// The hash of the segment `bytes` close, stored here; the last part finishes the file.
    fn cut_part(
        &self,
        session: &Session,
        st: &mut State,
        bytes: &[u8],
        last: bool,
    ) -> eyre::Result<[u8; 32]> {
        let writer = st
            .writer
            .as_mut()
            .ok_or_else(|| eyre::eyre!("session closed"))?;
        let mut segments = if last {
            writer.write(bytes)?
        } else {
            writer.write_more(bytes)?
        };
        if last {
            let writer = st
                .writer
                .take()
                .ok_or_else(|| eyre::eyre!("session closed"))?;
            let data_key = Zeroizing::new(*writer.data_key());
            let done = writer.finish()?;
            let file_key =
                Zeroizing::new(crate::backup::file_key(&self.cluster_key, &session.app_id));
            st.finished = Some(Finished {
                size: done.size,
                sha256: done.sha256,
                wrapped_key: wrap_key(&file_key, &session.app_id, &data_key)?,
            });
            segments.push(done.last);
        }
        let [segment] =
            <[_; 1]>::try_from(segments).map_err(|_| eyre::eyre!("a part makes one segment"))?;
        self.engine
            .put_segment(&hex::encode(segment.hash), &segment.bytes)?;
        Ok(segment.hash)
    }

    async fn complete_upload(
        &self,
        g: &Group<AppMachine>,
        app: &AppRecord,
        account: &str,
        upload: &str,
        now: u64,
    ) -> Result<Value, ApiError> {
        let key = format!("{}{COMPLETE_KEY}{upload}", crate::app::NODE_KEY_PREFIX);
        let Ok(session) = self.session(upload, &app.app_id, account, now) else {
            return replayed(g, &key, account).await;
        };
        let st = session.state.lock().await;
        if st.gone {
            drop(st);
            return replayed(g, &key, account).await;
        }
        if st.acked < session.parts {
            return Err(refusal(
                409,
                "UPLOAD_INCOMPLETE",
                "send every part first",
                st.acked,
            ));
        }
        self.finish_upload(g, &session, st, &key).await
    }

    /// Charges the file and commits its mark; the session ends unless the commit may yet land.
    async fn finish_upload(
        &self,
        g: &Group<AppMachine>,
        session: &Arc<Session>,
        mut st: tokio::sync::MutexGuard<'_, State>,
        key: &str,
    ) -> Result<Value, ApiError> {
        let delta =
            i64::try_from(session.size).map_err(|_| ApiError::internal("file too large"))?;
        if !st.charged {
            match self.count_file_bytes(&session.app_id, delta).await {
                Ok(()) => st.charged = true,
                Err(e) if e.status == 429 => {
                    drop(st);
                    self.end_session(session).await;
                    return Err(e);
                }
                Err(e) => return Err(e),
            }
        }
        let mark = mark_of(&st)?;
        match self
            .commit_mark(g, &session.app_id, mark, key, &session.account)
            .await
        {
            Ok(up) => {
                self.forget_session(session);
                st.gone = true;
                serde_json::to_value(up).map_err(|e| ApiError::internal(&e.to_string()))
            }
            Err(Upload::Unknown(e)) => Err(e),
            Err(Upload::Elsewhere(_)) => {
                self.forget_session(session);
                st.gone = true;
                Err(unknown())
            }
            Err(Upload::Refused(e)) => {
                self.forget_session(session);
                st.gone = true;
                Err(e)
            }
        }
    }

    fn forget_session(&self, session: &Session) {
        let _gone = (self.uploads.0.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&session.id);
    }

    /// Ends `session` and deletes its segments where they were pushed; the bytes of a session
    /// whose commit may yet land stay.
    async fn end_session(&self, session: &Session) {
        let mut st = session.state.lock().await;
        if std::mem::replace(&mut st.gone, true) {
            return;
        }
        self.forget_session(session);
        if !st.charged {
            let segments = st.segments.iter().map(hex::encode).collect();
            self.delete_files_later(
                &session.app_id,
                Stored {
                    segments,
                    ..Stored::default()
                },
            );
        }
    }

    fn end_later(&self, session: &Arc<Session>) {
        let Some(node) = self.this.lock().ok().and_then(|w| w.upgrade()) else {
            return;
        };
        let session = Arc::clone(session);
        drop(tokio::spawn(
            async move { node.end_session(&session).await },
        ));
    }

    /// Ends the sessions of `app_id`: this node no longer leads it.
    fn drop_sessions_of(&self, app_id: &str) {
        let held: Vec<Arc<Session>> = (self.uploads.0.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|s| s.app_id == app_id)
            .cloned()
            .collect();
        for session in &held {
            self.end_later(session);
        }
    }

    /// Ends the sessions that ran out or whose app this node no longer leads.
    pub(crate) fn expire_uploads(&self) {
        let now = self.app_clock().now_ms();
        let held: Vec<Arc<Session>> = (self.uploads.0.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for session in &held {
            let leads = self
                .hosted(&session.app_id)
                .is_some_and(|g| g.status().is_leader);
            if !leads
                || lapsed(
                    session.began_ms,
                    session.last_ms.load(Ordering::Relaxed),
                    now,
                )
            {
                self.end_later(session);
            }
        }
    }
}

/// The mark of a session whose parts are all in.
fn mark_of(st: &State) -> Result<FileMark, ApiError> {
    let done = st
        .finished
        .as_ref()
        .ok_or_else(|| ApiError::internal("upload not finished"))?;
    Ok(FileMark {
        id: hex::encode(file_id(&st.segments)),
        size: done.size,
        sha256: calimero_log::Bytes(done.sha256.to_vec()),
        wrapped_key: calimero_log::Bytes(done.wrapped_key.clone()),
        segments: st.segments.iter().map(hex::encode).collect(),
    })
}

/// The answer `key` committed, to the account that uploaded it; else the upload is unknown.
async fn replayed(g: &Group<AppMachine>, key: &str, account: &str) -> Result<Value, ApiError> {
    let found = g
        .read_strong(|m, _| {
            let up = uploaded(&m.component_replay(&Caller::System, key)?)?;
            let id: [u8; 32] = hex::decode(&up.mark.id).ok()?.try_into().ok()?;
            let by = m
                .state
                .component
                .blobs
                .records
                .get(&id)?
                .file
                .as_ref()?
                .uploader
                .clone();
            (by.as_deref() == Some(account)).then_some(up)
        })
        .await
        .map_err(|e| ApiError::unavailable(&e.to_string()))?;
    let up: Uploaded = found.ok_or_else(unknown)?;
    serde_json::to_value(up).map_err(|e| ApiError::internal(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_holds_four_sessions_and_a_node_thirty_two() {
        let four: Vec<(&str, u64)> = vec![("a", 1); 4];
        assert_eq!(admit(&four[..3], "a", 1), Ok(()));
        assert_eq!(admit(&four, "a", 1), Err(Refused::App));
        assert_eq!(admit(&four, "b", 1), Ok(()));
        let many: Vec<(&str, u64)> = (0..MAX_UPLOADS_PER_NODE)
            .map(|i| (if i % 2 == 0 { "a" } else { "b" }, 1))
            .collect();
        let few: Vec<(&str, u64)> = vec![("c", 1); 3];
        assert_eq!(admit(&few, "d", 1), Ok(()));
        assert_eq!(admit(&many[..31], "e", 1), Ok(()));
        assert_eq!(admit(&many, "e", 1), Err(Refused::Node));
    }

    #[test]
    fn a_leader_declares_at_most_sixteen_gib_in_flight() {
        let open = [("a", 8 << 30), ("b", 8 << 30)];
        assert_eq!(admit(&open[..1], "c", 8 << 30), Ok(()));
        assert_eq!(admit(&open, "c", 1), Err(Refused::InFlight));
    }

    #[test]
    fn a_session_lapses_when_idle_or_old() {
        let (idle, ttl) = (
            UPLOAD_IDLE.as_millis() as u64,
            UPLOAD_TTL.as_millis() as u64,
        );
        assert!(!lapsed(0, 0, idle - 1));
        assert!(lapsed(0, 0, idle));
        assert!(!lapsed(0, ttl - 2, ttl - 1));
        assert!(lapsed(0, ttl, ttl), "busy to the end, but six hours old");
    }

    #[test]
    fn a_file_is_one_part_per_segment_and_at_least_one() {
        let part = PART_BYTES as u64;
        assert_eq!(parts_of(0), 1);
        assert_eq!(parts_of(part), 1);
        assert_eq!(parts_of(part + 1), 2);
        assert_eq!(parts_of(2 * 1024 * 1024 * 1024), 128);
    }
}
