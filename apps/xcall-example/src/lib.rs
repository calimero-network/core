#![allow(clippy::len_without_is_empty)]

use calimero_sdk::{app, ContextId};
use calimero_storage::collections::Counter;

#[app::state(emits = Event)]
pub struct XCallExample {
    /// Bumped by `pong` — the declared `#[app::xcall]` entry point.
    counter: Counter,
    /// Bumped by `secret` — a method that is deliberately NOT an xcall entry
    /// point. Stays at zero when targeted by an xcall (the node's L3 gate
    /// denies it); only a direct call can move it.
    secret_counter: Counter,
}

#[app::event]
pub enum Event {
    PingSent {
        to_context: ContextId,
        method: String,
    },
    PongReceived {
        from_context: ContextId,
        counter: u64,
    },
}

#[app::logic]
impl XCallExample {
    #[app::init]
    pub fn init() -> XCallExample {
        XCallExample {
            counter: Counter::new(),
            secret_counter: Counter::new(),
        }
    }

    /// Send a ping to another context, dispatching an `xcall` to its `pong`
    /// entry point. `target_context` arrives base58-encoded and is parsed into
    /// a `ContextId` by the SDK.
    pub fn ping(&mut self, target_context: ContextId) -> app::Result<()> {
        self.xcall_to(target_context, "pong")
    }

    /// Send a ping that targets an arbitrary method on `target_context`. Used to
    /// exercise the L3 entry-point gate (e.g. targeting `secret`, which is not
    /// `#[app::xcall]`, must be denied by the node before it executes).
    pub fn ping_to(&mut self, target_context: ContextId, method: String) -> app::Result<()> {
        self.xcall_to(target_context, &method)
    }

    /// Receive a pong via `xcall`. Marked `#[app::xcall]`, so the node permits
    /// other contexts to invoke it (subject to the namespace boundary).
    ///
    /// Demonstrates caller provenance (L2a): `env::xcall_origin()` is set by the
    /// node from the calling context and cannot be forged by the caller, so this
    /// method (a) rejects direct/RPC calls that carry no origin, and (b) verifies
    /// the node-set origin matches the caller's self-reported `from_context` — a
    /// mismatch means a spoofed argument and is refused.
    #[app::xcall]
    pub fn pong(&mut self, from_context: ContextId) -> app::Result<()> {
        let origin = calimero_sdk::env::xcall_origin().map(ContextId::from);

        let Some(origin) = origin else {
            app::bail!("pong is xcall-only: no cross-context origin (direct call rejected)");
        };
        if origin != from_context {
            app::bail!(
                "xcall provenance mismatch: node-set origin {} != claimed from_context {}",
                origin,
                from_context
            );
        }

        self.counter.increment()?;
        let counter = self.counter.value()?;

        app::emit!(Event::PongReceived {
            from_context,
            counter,
        });

        app::log!(
            "pong from {} accepted; counter now {}",
            from_context,
            counter
        );

        Ok(())
    }

    /// A method that is intentionally NOT an `#[app::xcall]` entry point. A
    /// direct call works, but an `xcall` targeting it must be denied by the
    /// node's L3 gate before execution — so `secret_counter` stays at zero
    /// whenever it is reached via `xcall`.
    pub fn secret(&mut self, from_context: ContextId) -> app::Result<()> {
        let _ = from_context;
        self.secret_counter.increment()?;
        app::log!(
            "secret executed; secret_counter now {}",
            self.secret_counter.value()?
        );
        Ok(())
    }

    /// Current `pong` counter.
    pub fn get_counter(&self) -> app::Result<u64> {
        Ok(self.counter.value()?)
    }

    /// Current `secret` counter (should remain 0 when `secret` is only ever
    /// reached via a denied `xcall`).
    pub fn get_secret_counter(&self) -> app::Result<u64> {
        Ok(self.secret_counter.value()?)
    }

    /// Queue an `xcall` to `method` on `target_context`, passing this context's
    /// id as `from_context` so the target can compare it against the node-set
    /// `env::xcall_origin()`.
    fn xcall_to(&mut self, target_context: ContextId, method: &str) -> app::Result<()> {
        let current_context = ContextId::from(calimero_sdk::env::context_id());

        app::log!(
            "xcall '{}' from {} to {}",
            method,
            current_context,
            target_context
        );

        #[derive(calimero_sdk::serde::Serialize)]
        #[serde(crate = "calimero_sdk::serde")]
        struct Params {
            from_context: ContextId,
        }

        let params = calimero_sdk::serde_json::to_vec(&Params {
            from_context: current_context,
        })?;

        calimero_sdk::env::xcall(target_context.as_ref(), method, &params);

        app::emit!(Event::PingSent {
            to_context: target_context,
            method: method.to_owned(),
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    // `pong` is xcall-only: a direct call carries no `env::xcall_origin()`, so it
    // is rejected. (TestHost drives methods directly, with no xcall origin.)
    #[test]
    fn pong_rejects_direct_call() {
        let mut app = TestHost::new(XCallExample::init);
        let from = ContextId::from([7u8; 32]);
        assert!(
            app.call(|s| s.pong(from)).is_err(),
            "direct call to xcall-only pong must be rejected"
        );
        assert_eq!(app.view(|s| s.get_counter()).unwrap(), 0);
    }

    // `secret` is a normal method: a direct call works (only *xcalls* to it are
    // gated, by the node — not visible at this layer).
    #[test]
    fn secret_direct_call_increments() {
        let mut app = TestHost::new(XCallExample::init);
        let from = ContextId::from([7u8; 32]);
        app.call(|s| s.secret(from)).unwrap();
        app.call(|s| s.secret(from)).unwrap();
        assert_eq!(app.view(|s| s.get_secret_counter()).unwrap(), 2);
    }
}
