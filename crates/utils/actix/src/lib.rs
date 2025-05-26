use std::sync::OnceLock;

use eyre::bail;
use tokio::runtime::{Handle, RuntimeFlavor};

pub mod adapters;
pub mod lazy;
#[doc(hidden)]
pub mod macros;

pub use lazy::{LazyAddr, LazyRecipient};

static GLOBAL_RUNTIME: OnceLock<Handle> = OnceLock::new();

pub fn init_global_runtime() -> eyre::Result<()> {
    let handle = Handle::current();

    if handle.runtime_flavor() == RuntimeFlavor::CurrentThread {
        bail!("global runtime must not be a current-thread runtime");
    }

    if GLOBAL_RUNTIME.set(handle).is_err() {
        bail!("global runtime already initialized");
    }

    Ok(())
}

pub fn global_runtime() -> &'static Handle {
    GLOBAL_RUNTIME
        .get()
        .expect("global runtime not initialized")
}
