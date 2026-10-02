use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use actix::{Actor, Arbiter, Context, Handler, Message, Supervised};
use futures_util::join;

use super::{
    exit_on_panic, restart_on_panic, CrashLoopGuard, CRASH_LOOP_MAX_RESTARTS, CRASH_LOOP_WINDOW,
    PANIC_EXIT_CODE,
};

const CHILD_ENV: &str = "CALIMERO_SUPERVISE_TEST_CHILD"; // set when a test re-runs itself as the process under test

static RESTARTS: AtomicUsize = AtomicUsize::new(0);

struct Flaky {
    starts: usize,
}

impl Actor for Flaky {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        self.starts += 1;
    }
}

impl Supervised for Flaky {}

#[derive(Message)]
#[rtype("()")]
struct Panic;

impl Handler<Panic> for Flaky {
    type Result = ();

    fn handle(&mut self, _msg: Panic, _ctx: &mut Self::Context) {
        panic!("handler panic");
    }
}

struct Streaming;

impl Actor for Streaming {
    type Context = Context<Self>;

    crate::actor!(Streaming);
}

impl Handler<Panic> for Streaming {
    type Result = ();

    fn handle(&mut self, _msg: Panic, _ctx: &mut Self::Context) {
        panic!("handler panic");
    }
}

#[derive(Message)]
#[rtype(usize)]
struct Starts;

impl Handler<Starts> for Flaky {
    type Result = usize;

    fn handle(&mut self, _msg: Starts, _ctx: &mut Self::Context) -> usize {
        self.starts
    }
}

fn count_restart(_actor: &'static str) {
    let _ = RESTARTS.fetch_add(1, Ordering::Relaxed);
}

/// Re-runs `test` in a child process and asserts it ended through the panic exit path.
fn assert_child_exits(test: &str) {
    let module = module_path!().split_once("::").map_or("", |(_, rest)| rest);
    let output = Command::new(std::env::current_exe().expect("test binary path"))
        .args([&format!("{module}::{test}"), "--exact", "--nocapture"])
        .env(CHILD_ENV, "1")
        .output()
        .expect("run child test");

    assert_eq!(
        output.status.code(),
        Some(PANIC_EXIT_CODE),
        "child stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[actix::test]
async fn handler_panic_does_not_stop_the_actor() {
    let addr = restart_on_panic(&Arbiter::current(), count_restart, |_ctx| Flaky {
        starts: 0,
    });

    let (panicked, queued) = join!(addr.send(Panic), addr.send(Starts));
    assert!(panicked.is_err(), "the panicking request fails");
    assert_eq!(
        queued.ok(),
        Some(2),
        "a queued message survives and started() ran again"
    );
    assert_eq!(addr.send(Starts).await.ok(), Some(2));
    assert_eq!(RESTARTS.load(Ordering::Relaxed), 1);
}

#[actix::test]
async fn crash_loop_exits_the_process() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return assert_child_exits("crash_loop_exits_the_process");
    }

    let addr = restart_on_panic(&Arbiter::current(), count_restart, |_ctx| Flaky {
        starts: 0,
    });
    for _ in 0..CRASH_LOOP_MAX_RESTARTS {
        let _ = addr.send(Panic).await;
    }
    assert!(addr.send(Starts).await.is_ok(), "restarts up to the limit");

    let _ = addr.send(Panic).await;
    panic!("one restart past the limit should have exited the process");
}

#[actix::test]
async fn a_panic_with_no_sender_left_exits_the_process() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return assert_child_exits("a_panic_with_no_sender_left_exits_the_process");
    }

    let addr = restart_on_panic(&Arbiter::current(), count_restart, |_ctx| Flaky {
        starts: 0,
    });
    addr.do_send(Panic);
    drop(addr);
    actix::clock::sleep(Duration::from_secs(5)).await;
    panic!("an actor that cannot be restarted should have exited the process");
}

#[actix::test]
async fn exit_on_panic_exits_the_process() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return assert_child_exits("exit_on_panic_exits_the_process");
    }

    let addr = exit_on_panic(&Arbiter::current(), |_ctx| Flaky { starts: 0 });
    let _ = addr.send(Panic).await;
    panic!("a panic in an exit-on-panic actor should have exited the process");
}

#[actix::test]
async fn actor_macro_exits_the_process_on_panic() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return assert_child_exits("actor_macro_exits_the_process_on_panic");
    }

    let addr = Streaming.start();
    let _ = addr.send(Panic).await;
    panic!("a panic in an actor! actor should have exited the process");
}

#[test]
fn guard_trips_only_past_the_limit_within_the_window() {
    let mut guard = CrashLoopGuard::default();
    let now = Instant::now();

    for _ in 0..CRASH_LOOP_MAX_RESTARTS {
        assert!(!guard.record(now));
    }
    assert!(guard.record(now));
}

#[test]
fn restarts_older_than_the_window_are_forgotten() {
    let mut guard = CrashLoopGuard::default();
    let start = Instant::now();

    for _ in 0..CRASH_LOOP_MAX_RESTARTS {
        assert!(!guard.record(start));
    }
    let later = start + CRASH_LOOP_WINDOW + Duration::from_secs(1);
    assert!(!guard.record(later));
}
