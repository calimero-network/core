//! Binary entry point. Everything it serves is defined in the library, so the
//! contract tests exercise the same router this starts.

use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use calimero_account::AccountId;
use calimero_client_stub::{router, StubState};
use clap::Parser;
use eyre::bail;

/// The executor account the stub reports and requires warrants to name, when the
/// operator does not pick one. A fixed, obviously-fake value: a client that
/// hardcodes it instead of reading `GET .../intents` will fail the moment it
/// meets a real node, which is the useful outcome.
const DEFAULT_EXECUTOR_HEX: &str =
    "5748b0000000000000000000000000000000000000000000000000000000b075";

#[derive(Parser)]
#[command(
    name = "calimero-client-stub",
    about = "Serves the delegated-execution client contracts for client development"
)]
struct Args {
    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// The executor account this stub reports, as 64 hex characters.
    #[arg(long, default_value = DEFAULT_EXECUTOR_HEX)]
    executor_account: String,

    /// Report `canAuthorOnBehalf: false` and refuse intents, so a client can
    /// exercise the branch where it must tell the user to ask an admin for the
    /// grant. That is the default state of every real context, and the branch
    /// clients most often skip.
    #[arg(long)]
    refuse_authorship: bool,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let args = Args::parse();

    let raw = hex::decode(&args.executor_account)?;
    let Ok(raw) = <[u8; 32]>::try_from(raw.as_slice()) else {
        bail!("--executor-account must be exactly 64 hex characters (32 bytes)");
    };

    let state = Arc::new(StubState {
        executor_account: AccountId::from(raw),
        executor_account_hex: args.executor_account.clone(),
        refuse_authorship: args.refuse_authorship,
        challenges: Mutex::new(Vec::new()),
        spent: Mutex::new(Vec::new()),
        tokens: AtomicU64::new(1),
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!("calimero-client-stub listening on http://{addr}");
    println!("  executor account : {}", args.executor_account);
    println!(
        "  authorship       : {}",
        if args.refuse_authorship {
            "REFUSED (--refuse-authorship)"
        } else {
            "granted"
        }
    );
    println!("\nThis is NOT a security boundary: signatures are not verified.\n");

    axum::serve(listener, router(state)).await?;
    Ok(())
}
