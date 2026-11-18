//! Synchronous xcalls as futures, and the executor that batches them.
//!
//! A call or view of another app ([`crate::env::xcall_call`], [`crate::env::xcall_view`], a
//! generated client's methods) is a [`Pending`] future. Awaiting it inside an `async fn` app
//! method (or [`block_on`]) does not wait on its own: the executor polls the method until
//! every future it awaits is waiting on an xcall, sends those xcalls to the node as one
//! `xcall.batch`, hands each future its answer and polls again. Futures awaited one after
//! another make one batch each; futures joined with [`crate::app::join!`] or [`join_all`]
//! are polled in the same round, so their xcalls go in one batch and the node runs them at
//! once: the run waits for the slowest, not for their sum.
//!
//! The executor is single-threaded and deterministic: a batch lists its requests in the
//! order the method polled them, so a re-run makes the same batches with the same keys.
//! Only these futures can make a run wait; a method pending on anything else is a bug and
//! traps with a message.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::cell::RefCell;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

use crate::error::{AppError, Result};

/// Whether a synchronous xcall is a call (logged in the target) or a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The target's call, committed in its own log before this run continues.
    Call,
    /// A read of the target's latest committed state.
    View,
}

/// One synchronous xcall, as a batch sends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// A call or a view.
    pub kind: Kind,
    /// The target app.
    pub app: String,
    /// The target method.
    pub method: String,
    /// The JSON arguments.
    pub args: Vec<u8>,
}

/// An xcall's raw answer: the target's JSON result, or the failure as
/// `XcallFailed` (the target's app error as `{code, fields}`, else `{message}`).
pub type Answer = Result<Vec<u8>>;

/// A started synchronous xcall: resolves to `decode` of its answer once the executor has
/// sent the batch it is in. Dropping one before its batch is sent leaves it out.
#[must_use = "an xcall is sent only when it is awaited"]
pub struct Pending<T> {
    state: State,
    decode: fn(Answer) -> T,
}

enum State {
    /// Not polled yet: the request it will queue.
    New(Request),
    /// Its arguments did not serialize: answered without reaching the node.
    Failed(AppError),
    /// Queued or sent in run `generation` as slot `slot`.
    Queued { generation: u64, slot: usize },
    /// Its answer was handed out.
    Done,
}

impl<T> Pending<T> {
    /// A call of `method` of `app` with `args` (serialized now), resolving to
    /// `decode(answer)`.
    pub fn call<A: Serialize + ?Sized>(
        app: &str,
        method: &str,
        args: &A,
        decode: fn(Answer) -> T,
    ) -> Self {
        Self::new(Kind::Call, app, method, args, decode)
    }

    /// A view, as [`Self::call`].
    pub fn view<A: Serialize + ?Sized>(
        app: &str,
        method: &str,
        args: &A,
        decode: fn(Answer) -> T,
    ) -> Self {
        Self::new(Kind::View, app, method, args, decode)
    }

    fn new<A: Serialize + ?Sized>(
        kind: Kind,
        app: &str,
        method: &str,
        args: &A,
        decode: fn(Answer) -> T,
    ) -> Self {
        let state = match serde_json::to_vec(args) {
            Ok(args) => State::New(Request {
                kind,
                app: app.to_owned(),
                method: method.to_owned(),
                args,
            }),
            Err(e) => State::Failed(AppError::with_fields(
                "BadArgs",
                json!({ "message": e.to_string() }),
            )),
        };
        Self { state, decode }
    }
}

impl<T> Future for Pending<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<T> {
        let this = self.get_mut();
        match std::mem::replace(&mut this.state, State::Done) {
            State::New(request) => {
                let (generation, slot) = EXEC.with(|e| e.borrow_mut().queue(request));
                this.state = State::Queued { generation, slot };
                Poll::Pending
            }
            State::Failed(e) => Poll::Ready((this.decode)(Err(e))),
            State::Queued { generation, slot } => {
                match EXEC.with(|e| e.borrow_mut().take(generation, slot)) {
                    Some(answer) => Poll::Ready((this.decode)(answer)),
                    None => {
                        this.state = State::Queued { generation, slot };
                        Poll::Pending
                    }
                }
            }
            State::Done => panic!("an xcall future was polled after it answered"),
        }
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if let State::Queued { generation, slot } = self.state {
            // `try_with`: a future dropped while the thread's executor is torn down.
            let _ = EXEC.try_with(|e| {
                if let Ok(mut e) = e.try_borrow_mut() {
                    e.forget(generation, slot);
                }
            });
        }
    }
}

/// The decoding of an answer the SDK's own functions use: the JSON result as `T` (an empty
/// result is `null`), `BadOutput` when it is not one, a failure unchanged.
///
/// # Errors
///
/// The answer's failure, or `BadOutput`.
pub fn decode<T: DeserializeOwned>(answer: Answer) -> Result<T> {
    let out = answer?;
    let out = if out.is_empty() {
        b"null".to_vec()
    } else {
        out
    };
    serde_json::from_slice(&out)
        .map_err(|e| AppError::with_fields("BadOutput", json!({ "message": e.to_string() })))
}

enum Slot {
    Queued(Request),
    Sent,
    Answered(Answer),
    Taken,
    Dropped,
}

/// The executor's state for one run: the xcalls futures queued and the answers waiting for
/// them. Cleared when the outermost [`block_on`] starts; `generation` tells a future kept
/// past its run from one of this run.
#[derive(Default)]
struct Exec {
    depth: u32,
    generation: u64,
    slots: Vec<Slot>,
    /// Slots queued since the last batch, in the order they were polled.
    queued: Vec<usize>,
}

thread_local! {
    static EXEC: RefCell<Exec> = RefCell::default();
}

impl Exec {
    fn queue(&mut self, request: Request) -> (u64, usize) {
        assert!(
            self.depth > 0,
            "an xcall future must be awaited inside an `async fn` app method or `app::block_on`"
        );
        self.slots.push(Slot::Queued(request));
        let slot = self.slots.len() - 1;
        self.queued.push(slot);
        (self.generation, slot)
    }

    fn take(&mut self, generation: u64, slot: usize) -> Option<Answer> {
        assert!(
            generation == self.generation,
            "an xcall future was kept past the run that started it"
        );
        let entry = self.slots.get_mut(slot)?;
        match std::mem::replace(entry, Slot::Taken) {
            Slot::Answered(answer) => Some(answer),
            other => {
                *entry = other;
                None
            }
        }
    }

    fn forget(&mut self, generation: u64, slot: usize) {
        if generation != self.generation {
            return;
        }
        if let Some(entry) = self.slots.get_mut(slot) {
            if matches!(entry, Slot::Queued(_) | Slot::Answered(_)) {
                *entry = Slot::Dropped;
            }
        }
    }

    /// The requests queued since the last batch (left out: those dropped since), marked sent.
    fn next_batch(&mut self) -> (Vec<usize>, Vec<Request>) {
        let mut slots = Vec::new();
        let mut requests = Vec::new();
        for slot in std::mem::take(&mut self.queued) {
            if let Some(entry) = self.slots.get_mut(slot) {
                if let Slot::Queued(request) = std::mem::replace(entry, Slot::Sent) {
                    slots.push(slot);
                    requests.push(request);
                } else {
                    *entry = Slot::Dropped;
                }
            }
        }
        (slots, requests)
    }

    fn answer(&mut self, slots: Vec<usize>, answers: Vec<Answer>) {
        let mut answers = answers.into_iter();
        for slot in slots {
            let answer = answers.next().unwrap_or_else(|| {
                Err(AppError::with_fields(
                    "XcallFailed",
                    json!({ "message": "the host gave no answer to this xcall" }),
                ))
            });
            if let Some(entry @ Slot::Sent) = self.slots.get_mut(slot) {
                *entry = Slot::Answered(answer);
            }
        }
    }
}

/// Leaves the executor when [`block_on`] returns (or unwinds).
struct Entered;

impl Drop for Entered {
    fn drop(&mut self) {
        let _ = EXEC.try_with(|e| {
            if let Ok(mut e) = e.try_borrow_mut() {
                e.depth -= 1;
                if e.depth == 0 {
                    e.slots.clear();
                    e.queued.clear();
                }
            }
        });
    }
}

/// Runs `fut` to completion in this run: polls it, and whenever it can go no further sends
/// the xcalls it waits on as one batch, then polls again. `#[app::logic]` runs every
/// `async fn` method with it; call it yourself to await xcalls in a plain `fn`.
///
/// # Panics
///
/// When `fut` is pending without waiting on an xcall (it awaits something only another
/// executor would wake): a run has nothing else to wait for, so this is a bug.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    EXEC.with(|e| {
        let mut e = e.borrow_mut();
        if e.depth == 0 {
            e.generation += 1;
            e.slots.clear();
            e.queued.clear();
        }
        e.depth += 1;
    });
    let _entered = Entered;
    let mut fut = core::pin::pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
        let (slots, requests) = EXEC.with(|e| e.borrow_mut().next_batch());
        assert!(
            !requests.is_empty(),
            "app::block_on: the method is pending but waits on no xcall; only the SDK's xcall \
             futures can be awaited in a run"
        );
        let answers = crate::env::xcall_batch(&requests);
        EXEC.with(|e| e.borrow_mut().answer(slots, answers));
    }
}

/// A joined future's state: running, done with its output, or its output handed out.
enum MaybeDone<F: Future> {
    Running(Pin<Box<F>>),
    Done(F::Output),
    Gone,
}

// The future is boxed and the output is never pinned, so moving a `MaybeDone` is fine.
impl<F: Future> Unpin for MaybeDone<F> {}

impl<F: Future> MaybeDone<F> {
    fn new(fut: F) -> Self {
        Self::Running(Box::pin(fut))
    }

    /// Polls it if it is still running; whether it is done.
    fn poll_done(&mut self, cx: &mut Context<'_>) -> bool {
        if let Self::Running(fut) = self {
            match fut.as_mut().poll(cx) {
                Poll::Ready(out) => *self = Self::Done(out),
                Poll::Pending => return false,
            }
        }
        true
    }

    fn output(&mut self) -> F::Output {
        match std::mem::replace(self, Self::Gone) {
            Self::Done(out) => out,
            // SAFETY: guaranteed by the joins, which take an output once, after every
            // future reported done.
            Self::Running(_) | Self::Gone => unreachable!("a joined future's output taken early"),
        }
    }
}

/// The futures of a `Vec`, run together: [`join_all`].
#[must_use = "futures do nothing unless awaited"]
pub struct JoinAll<F: Future> {
    futs: Vec<MaybeDone<F>>,
}

/// Runs every future of `futs` together and answers their outputs in order: each round
/// polls all of them, so their xcalls go in one batch.
pub fn join_all<I>(futs: I) -> JoinAll<I::Item>
where
    I: IntoIterator,
    I::Item: Future,
{
    JoinAll {
        futs: futs.into_iter().map(MaybeDone::new).collect(),
    }
}

impl<F: Future> Future for JoinAll<F> {
    type Output = Vec<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut done = true;
        for fut in &mut this.futs {
            done &= fut.poll_done(cx);
        }
        if !done {
            return Poll::Pending;
        }
        Poll::Ready(this.futs.iter_mut().map(MaybeDone::output).collect())
    }
}

macro_rules! joins {
    ($($name:ident: $($F:ident $f:ident),+;)+) => {$(
        #[doc = concat!("Futures run together: [`crate::app::join!`] with as many arguments as `", stringify!($name), "` has.")]
        #[doc(hidden)]
        #[must_use = "futures do nothing unless awaited"]
        pub struct $name<$($F: Future),+> {
            $($f: MaybeDone<$F>,)+
        }

        impl<$($F: Future),+> $name<$($F),+> {
            #[doc(hidden)]
            #[allow(clippy::too_many_arguments, reason = "one argument per joined future, up to 8")]
            pub fn new($($f: $F),+) -> Self {
                Self { $($f: MaybeDone::new($f),)+ }
            }
        }

        impl<$($F: Future),+> Future for $name<$($F),+> {
            type Output = ($($F::Output,)+);

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let this = self.get_mut();
                let mut done = true;
                $(done &= this.$f.poll_done(cx);)+
                if !done {
                    return Poll::Pending;
                }
                Poll::Ready(($(this.$f.output(),)+))
            }
        }
    )+};
}

joins! {
    Join2: A a, B b;
    Join3: A a, B b, C c;
    Join4: A a, B b, C c, D d;
    Join5: A a, B b, C c, D d, E e;
    Join6: A a, B b, C c, D d, E e, G g;
    Join7: A a, B b, C c, D d, E e, G g, H h;
    Join8: A a, B b, C c, D d, E e, G g, H h, I i;
}

/// Awaits two to eight futures together and answers their outputs as a tuple, in order:
/// their xcalls go in one batch, which the node runs at once. Inside an `async` method:
///
/// ```ignore
/// let (events, free) = app::join!(cal.my_events(), rooms.free(&day));
/// let (events, free) = (events?, free?);
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __app_join {
    ($a:expr, $b:expr $(,)?) => {
        $crate::xcall::Join2::new($a, $b).await
    };
    ($a:expr, $b:expr, $c:expr $(,)?) => {
        $crate::xcall::Join3::new($a, $b, $c).await
    };
    ($a:expr, $b:expr, $c:expr, $d:expr $(,)?) => {
        $crate::xcall::Join4::new($a, $b, $c, $d).await
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr $(,)?) => {
        $crate::xcall::Join5::new($a, $b, $c, $d, $e).await
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr $(,)?) => {
        $crate::xcall::Join6::new($a, $b, $c, $d, $e, $f).await
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr $(,)?) => {
        $crate::xcall::Join7::new($a, $b, $c, $d, $e, $f, $g).await
    };
    ($a:expr, $b:expr, $c:expr, $d:expr, $e:expr, $f:expr, $g:expr, $h:expr $(,)?) => {
        $crate::xcall::Join8::new($a, $b, $c, $d, $e, $f, $g, $h).await
    };
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::env::mock;

    /// Answers every xcall with `"<app>.<method>"`, and fails app `bad`'s.
    fn answering() {
        mock::reset();
        mock::with_mut(|s| {
            s.sync_xcalls = Some(Arc::new(|r: &Request| {
                if r.app == "bad" {
                    return Err(r#"{"code":"Nope","fields":{}}"#.to_owned());
                }
                Ok(format!("\"{}.{}\"", r.app, r.method).into_bytes())
            }));
        });
    }

    /// The batches sent so far, as `app.method` per request.
    fn batches() -> Vec<Vec<String>> {
        mock::with_mut(|s| {
            s.sync_batches
                .iter()
                .map(|b| {
                    b.iter()
                        .map(|r| format!("{}.{}", r.app, r.method))
                        .collect()
                })
                .collect()
        })
    }

    fn view(app: &str, method: &str) -> Pending<Result<String>> {
        crate::env::xcall_view(app, method, &())
    }

    #[test]
    fn joined_xcalls_go_in_one_batch_and_answer_in_order() {
        answering();
        let (a, b) = block_on(async { crate::app::join!(view("a", "x"), view("b", "y")) });
        assert_eq!(
            (a.unwrap(), b.unwrap()),
            ("a.x".to_owned(), "b.y".to_owned())
        );
        assert_eq!(batches(), vec![vec!["a.x", "b.y"]]);
    }

    #[test]
    fn awaits_in_sequence_make_a_batch_each() {
        answering();
        let both = block_on(async {
            let a = view("a", "x").await?;
            let b = view("b", a.as_str()).await?;
            Ok::<_, AppError>(b)
        });
        assert_eq!(both.unwrap(), "b.a.x");
        assert_eq!(batches(), vec![vec!["a.x"], vec!["b.a.x"]]);
    }

    #[test]
    fn nested_awaits_inside_a_join_batch_round_by_round() {
        answering();
        let chain = async {
            let first = view("a", "one").await?;
            view("a", &format!("after-{first}")).await
        };
        let (chain, single) = block_on(async { crate::app::join!(chain, view("b", "two")) });
        assert_eq!(chain.unwrap(), "a.after-a.one");
        assert_eq!(single.unwrap(), "b.two");
        // Round one: both first xcalls; round two: the chain's second.
        assert_eq!(
            batches(),
            vec![vec!["a.one", "b.two"], vec!["a.after-a.one"]]
        );
    }

    #[test]
    fn join_all_batches_every_future_and_keeps_failures_in_place() {
        answering();
        let all = block_on(join_all(["a", "bad", "c"].map(|app| view(app, "m"))));
        assert_eq!(all[0].as_deref().unwrap(), "a.m");
        let e = all[1].as_ref().unwrap_err();
        assert_eq!(e.code, "XcallFailed");
        assert_eq!(e.fields["code"], "Nope");
        assert_eq!(all[2].as_deref().unwrap(), "c.m");
        assert_eq!(batches(), vec![vec!["a.m", "bad.m", "c.m"]]);
    }

    #[test]
    fn a_dropped_future_is_left_out_and_bad_args_never_reach_the_node() {
        answering();
        let out = block_on(async {
            drop(view("gone", "m"));
            let mut map = std::collections::BTreeMap::new();
            let _ = map.insert(vec![1u8], 1u8);
            let bad: Result<String> = crate::env::xcall_call("a", "m", &map).await;
            let kept = view("kept", "m").await;
            (bad, kept)
        });
        assert_eq!(out.0.unwrap_err().code, "BadArgs");
        assert_eq!(out.1.unwrap(), "kept.m");
        assert_eq!(batches(), vec![vec!["kept.m"]]);
    }

    #[test]
    #[should_panic(expected = "waits on no xcall")]
    fn a_method_pending_on_anything_else_traps() {
        answering();
        block_on(core::future::pending::<()>());
    }

    #[test]
    #[should_panic(expected = "inside an `async fn` app method")]
    fn an_xcall_polled_outside_the_executor_traps() {
        answering();
        let mut fut = core::pin::pin!(view("a", "x"));
        let _ = fut.as_mut().poll(&mut Context::from_waker(Waker::noop()));
    }
}
