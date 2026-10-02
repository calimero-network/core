//! Starts actors so a handler panic restarts them or exits the process, never leaving a dead mailbox.
//! actix's `Supervisor` does not catch panics, so this catches them around the actor's poll instead.

use std::collections::VecDeque;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::task::{self, Poll};
use std::time::{Duration, Instant};

use actix::dev::{channel, ContextFut};
use actix::{Actor, Addr, ArbiterHandle, Context, Supervised};
use tracing::error;

#[cfg(test)]
#[path = "supervise_tests.rs"]
mod tests;

pub const CRASH_LOOP_MAX_RESTARTS: usize = 5; // restarts tolerated within the window; one more exits
pub const CRASH_LOOP_WINDOW: Duration = Duration::from_secs(1800); // trips on a panic every tick of up to a 5 minute interval
pub const PANIC_EXIT_CODE: i32 = 70; // EX_SOFTWARE in sysexits.h
#[doc(hidden)]
pub const MAILBOX_CAPACITY: usize = 16; // actix's default, which it keeps private

/// Counts recent restarts of one actor and reports when they form a crash loop.
#[derive(Debug, Default)]
pub struct CrashLoopGuard {
    restarts: VecDeque<Instant>,
}

impl CrashLoopGuard {
    /// Records a restart at `now`; true once more than the allowed number fall within the window.
    pub fn record(&mut self, now: Instant) -> bool {
        while self
            .restarts
            .front()
            .is_some_and(|at| now.duration_since(*at) > CRASH_LOOP_WINDOW)
        {
            let _ = self.restarts.pop_front();
        }
        self.restarts.push_back(now);
        self.restarts.len() > CRASH_LOOP_MAX_RESTARTS
    }
}

/// Starts `A` on `arbiter`, restarting it in place after a panic: the actor value and its fields are kept,
/// the message being handled and futures spawned on its context are dropped, and `started()` runs again.
pub fn restart_on_panic<A, F>(
    arbiter: &ArbiterHandle,
    on_restart: fn(&'static str),
    f: F,
) -> Addr<A>
where
    A: Actor<Context = Context<A>> + Supervised,
    F: FnOnce(&mut Context<A>) -> A + Send + 'static,
{
    start(
        arbiter,
        Some(Restarts {
            restart: ContextFut::restart,
            on_restart,
            guard: CrashLoopGuard::default(),
            total: 0,
        }),
        f,
    )
}

/// Starts `A` on `arbiter`, exiting the process if it ever panics. For actors whose in-flight work
/// cannot be dropped halfway without leaving state that only a fresh process rebuilds.
pub fn exit_on_panic<A, F>(arbiter: &ArbiterHandle, f: F) -> Addr<A>
where
    A: Actor<Context = Context<A>>,
    F: FnOnce(&mut Context<A>) -> A + Send + 'static,
{
    start(arbiter, None, f)
}

/// Logs that `A` panicked and exits, leaving the restart to the service manager.
#[doc(hidden)]
pub fn exit_after_panic<A>() -> ! {
    error!(
        actor = actor_name::<A>(),
        exit_code = PANIC_EXIT_CODE,
        "actor panicked and cannot continue; exiting so the node is restarted"
    );
    std::process::exit(PANIC_EXIT_CODE)
}

fn actor_name<A>() -> &'static str {
    let name = std::any::type_name::<A>();
    name.rsplit("::").next().unwrap_or(name)
}

fn start<A, F>(arbiter: &ArbiterHandle, restarts: Option<Restarts<A>>, f: F) -> Addr<A>
where
    A: Actor<Context = Context<A>>,
    F: FnOnce(&mut Context<A>) -> A + Send + 'static,
{
    let (tx, rx) = channel::channel(MAILBOX_CAPACITY);
    let _ignored = arbiter.spawn_fn(move || {
        let mut ctx = Context::with_receiver(rx);
        let act = f(&mut ctx);
        let fut = ctx.into_future(act);
        let _handle = actix::spawn(Guarded {
            fut,
            restarts,
            settled: false,
        });
    });
    Addr::new(tx)
}

struct Restarts<A: Actor<Context = Context<A>>> {
    restart: fn(&mut ContextFut<A, Context<A>>) -> bool,
    on_restart: fn(&'static str),
    guard: CrashLoopGuard,
    total: u64,
}

struct Guarded<A: Actor<Context = Context<A>>> {
    fut: ContextFut<A, Context<A>>,
    restarts: Option<Restarts<A>>,
    settled: bool, // a poll has returned Pending, so the startup wait queue (e.g. `Lazy::init`) has run
}

impl<A: Actor<Context = Context<A>>> Guarded<A> {
    /// Restarts the actor after a panic, or exits when it cannot be restarted.
    fn recover(&mut self) {
        let Some(restarts) = self.restarts.as_mut().filter(|_| self.settled) else {
            exit_after_panic::<A>()
        };
        let actor = actor_name::<A>();
        if restarts.guard.record(Instant::now()) {
            error!(
                actor,
                window_secs = CRASH_LOOP_WINDOW.as_secs(),
                "actor is crash-looping"
            );
            exit_after_panic::<A>()
        }
        let restart = restarts.restart;
        let fut = &mut self.fut;
        if !catch_unwind(AssertUnwindSafe(|| restart(fut))).unwrap_or(false) {
            error!(actor, "actor could not be restarted");
            exit_after_panic::<A>()
        }
        restarts.total += 1;
        (restarts.on_restart)(actor);
        error!(
            actor,
            restarts = restarts.total,
            "actor panicked and was restarted"
        );
    }
}

impl<A: Actor<Context = Context<A>>> Future for Guarded<A> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        loop {
            match catch_unwind(AssertUnwindSafe(|| Pin::new(&mut this.fut).poll(cx))) {
                Ok(poll) => {
                    this.settled |= poll.is_pending();
                    return poll;
                }
                Err(_panic) => this.recover(),
            }
        }
    }
}
