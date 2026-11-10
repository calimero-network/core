//! View subscriptions (PoC): a watch stream (`GET /apps/{app}/events?watch=true`), the
//! watches registered on it (`POST /apps/{app}/watches`), and [`Client::watch`], which
//! keeps one watch going across reconnects.
//!
//! A notice carries the watch id and an entry index, never the view's result: the client
//! re-reads the view itself, `Consistency::Stale { min_index: index }`.

use std::pin::Pin;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{path_segment, Client, Error, Result};

/// One SSE frame: `(id, event, data)`.
type Frames = Pin<Box<dyn Stream<Item = (String, String, Value)> + Send>>;

/// A watch the node took: its id and the index its view read at.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WatchAdded {
    pub sub: String,
    pub index: u64,
}

/// What [`Client::watch`] yields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchNotice {
    /// Something the view read changed at `index` (or, after a reconnect the node could not
    /// resume, may have): read the view again at `index` or later.
    Changed { index: u64 },
    /// The node dropped the watch (the caller lost access, or the view no longer reads a
    /// bounded set); nothing follows.
    Ended,
}

/// An open watch stream.
pub struct WatchStream {
    /// The stream's id, which watches are registered on.
    pub id: String,
    /// The frames after the first: `view-changed` (`id` `<index>:<seq>`, data `{sub,
    /// index}`) and `watch-ended` (data `{sub}`).
    pub frames: Frames,
}

/// One watch kept going by [`Client::watch`].
pub struct Watch<'a> {
    pub sub: String,
    /// The index the view read at when the watch was taken.
    pub index: u64,
    pub notices: Pin<Box<dyn Stream<Item = WatchNotice> + Send + 'a>>,
}

/// How long [`Client::watch`] waits before opening a dropped stream again.
const REOPEN_AFTER: Duration = Duration::from_millis(500);
/// Times in a row [`Client::watch`] tries to open a dropped stream before it ends.
const REOPEN_TRIES: u32 = 20;

impl Client {
    /// Opens a watch stream on `app`, or resumes `(stream, last index)` (the node sends
    /// again what that stream missed after `last`). A stream the node no longer holds is
    /// answered with a new one: [`WatchStream::id`] then differs, and its watches are gone.
    ///
    /// # Errors
    /// The node refused the stream, or it ended before naming itself.
    pub async fn watch_stream(
        &self,
        app: &str,
        resume: Option<(&str, u64)>,
    ) -> Result<WatchStream> {
        let mut path = format!("apps/{}/events?watch=true", path_segment(app));
        if let Some((stream, last)) = resume {
            path.push_str(&format!("&stream={}&last={last}", path_segment(stream)));
        }
        let mut frames: Frames = Box::pin(self.sse(&path).await?);
        match frames.next().await {
            Some((_, kind, data)) if kind == "stream" => {
                let id = data["stream"].as_str().unwrap_or_default().to_owned();
                Ok(WatchStream { id, frames })
            }
            _ => Err(Error::Transport(eyre::eyre!(
                "the watch stream did not name itself"
            ))),
        }
    }

    /// Watches `view(args)` on stream `stream`: the node runs the view as this client's
    /// caller and keeps what it read.
    ///
    /// # Errors
    /// `NOT_SUBSCRIBABLE`, `NOT_A_VIEW`, `READ_SET_UNBOUNDED`, `TOO_MANY_WATCHES`,
    /// `UNKNOWN_STREAM`, or whatever the view itself answers (its access refusals included).
    pub async fn add_watch(
        &self,
        app: &str,
        stream: &str,
        view: &str,
        args: Value,
    ) -> Result<WatchAdded> {
        let v = self
            .post(
                &format!("apps/{}/watches", path_segment(app)),
                json!({ "stream": stream, "view": view, "args": args }),
            )
            .await?;
        Ok(serde_json::from_value(v).map_err(eyre::Report::from)?)
    }

    /// Stops watch `sub`.
    ///
    /// # Errors
    /// `UNKNOWN_WATCH` for a watch this caller does not hold.
    pub async fn remove_watch(&self, app: &str, sub: &str) -> Result<()> {
        let _ = self
            .delete(&format!(
                "apps/{}/watches/{}",
                path_segment(app),
                path_segment(sub)
            ))
            .await?;
        Ok(())
    }

    /// Watches one view on a stream of its own, and keeps it going: when the connection
    /// drops it resumes the stream (missed notices are sent again); when the node no longer
    /// holds it, it registers the watch on a new stream and yields
    /// [`WatchNotice::Changed`] with the index the new watch read at, so the caller re-reads.
    ///
    /// # Errors
    /// As [`Self::watch_stream`] and [`Self::add_watch`], for the first stream and watch.
    pub async fn watch(&self, app: &str, view: &str, args: Value) -> Result<Watch<'_>> {
        let stream = self.watch_stream(app, None).await?;
        let added = self.add_watch(app, &stream.id, view, args.clone()).await?;
        let state = Keep {
            client: self,
            app: app.to_owned(),
            view: view.to_owned(),
            args,
            stream_id: stream.id,
            frames: Some(stream.frames),
            sub: added.sub.clone(),
            last: added.index,
            done: false,
        };
        let notices = futures_util::stream::unfold(state, |mut keep| async move {
            let notice = keep.next().await?;
            Some((notice, keep))
        });
        Ok(Watch {
            sub: added.sub,
            index: added.index,
            notices: Box::pin(notices),
        })
    }
}

/// [`Client::watch`]'s state.
struct Keep<'a> {
    client: &'a Client,
    app: String,
    view: String,
    args: Value,
    stream_id: String,
    frames: Option<Frames>,
    sub: String,
    /// The index of the last notice (or of the watch) seen.
    last: u64,
    done: bool,
}

impl Keep<'_> {
    async fn next(&mut self) -> Option<WatchNotice> {
        if self.done {
            return None;
        }
        loop {
            let Some(frames) = &mut self.frames else {
                if let Some(hint) = self.reopen().await {
                    return Some(hint);
                }
                if self.done {
                    return None;
                }
                continue;
            };
            let Some((_, kind, data)) = frames.next().await else {
                self.frames = None;
                continue;
            };
            if data["sub"].as_str() != Some(self.sub.as_str()) {
                continue;
            }
            match kind.as_str() {
                "view-changed" => {
                    let index = data["index"].as_u64().unwrap_or(self.last);
                    self.last = self.last.max(index);
                    return Some(WatchNotice::Changed { index });
                }
                "watch-ended" => {
                    self.done = true;
                    return Some(WatchNotice::Ended);
                }
                _ => {}
            }
        }
    }

    /// Opens the stream again; on a new stream, takes the watch again and answers the
    /// re-read hint. Ends the watch after [`REOPEN_TRIES`] failures.
    async fn reopen(&mut self) -> Option<WatchNotice> {
        for _ in 0..REOPEN_TRIES {
            tokio::time::sleep(REOPEN_AFTER).await;
            let resume = Some((self.stream_id.as_str(), self.last));
            let Ok(stream) = self.client.watch_stream(&self.app, resume).await else {
                continue;
            };
            if stream.id == self.stream_id {
                self.frames = Some(stream.frames);
                return None;
            }
            let added = self
                .client
                .add_watch(&self.app, &stream.id, &self.view, self.args.clone())
                .await;
            let Ok(added) = added else {
                self.done = true;
                return Some(WatchNotice::Ended);
            };
            self.stream_id = stream.id;
            self.frames = Some(stream.frames);
            self.sub = added.sub;
            self.last = self.last.max(added.index);
            return Some(WatchNotice::Changed { index: added.index });
        }
        self.done = true;
        Some(WatchNotice::Ended)
    }
}
