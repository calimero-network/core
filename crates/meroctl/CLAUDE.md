# meroctl - CLI Tool

Command-line interface for managing Calimero nodes, apps, contexts, and blobs.

- **Binary**: `meroctl`
- **Entry**: `src/main.rs`
- **Frameworks**: clap (CLI), tokio (async)

## Build & Run

```bash
cargo build -p meroctl
cargo build -p meroctl --release
cargo run -p meroctl -- --node node1 context ls
cargo test -p meroctl
```

## CLI Structure

```
meroctl --node <name> <subcommand>
├── app           install | list | get | uninstall
├── context       create | delete | list | get | invite | join | identity
├── blob          upload | download | list | delete
├── call          # Call a context method
├── peers         # Peer management
└── node          # Node info
```

## File Layout

```
src/
├── main.rs              # Entry point
├── cli.rs               # Root command
├── cli/
│   ├── app.rs           # App subcommands (declares mod)
│   ├── app/install.rs   # Install command
│   ├── app/list.rs
│   ├── context.rs       # Context subcommands (declares mod)
│   ├── context/create.rs
│   ├── context/identity/
│   ├── blob.rs
│   ├── blob/
│   ├── call.rs
│   ├── peers.rs
│   └── validation.rs
├── client.rs            # HTTP client wrapper
├── output.rs            # JSON/table output formatting
├── output/              # Per-resource output formatters
├── common.rs            # Shared utilities
├── config.rs
├── auth.rs
└── defaults.rs
```

## Patterns

### Subcommand Module Pattern

```rust
// src/cli/app.rs — parent declares children
use clap::Subcommand;

mod get;
mod install;
mod list;
mod uninstall;

#[derive(Debug, Subcommand)]
pub enum AppSubcommand {
    Install(install::InstallCommand),
    List(list::ListCommand),
    Get(get::GetCommand),
    Uninstall(uninstall::UninstallCommand),
}
```

### Command Implementation

```rust
// src/cli/app/install.rs
use clap::Parser;

#[derive(Debug, Parser)]
pub struct InstallCommand {
    #[clap(long, short)]
    path: Option<Utf8PathBuf>,
    #[clap(long, short)]
    url: Option<Url>,
}

impl InstallCommand {
    pub async fn run(self, environment: &Environment) -> Result<(), CliError> {
        // ...
    }
}
```

## Key Files

| File | Purpose |
|---|---|
| `src/cli.rs` | Root command, global args |
| `src/client.rs` | HTTP client wrapper |
| `src/output.rs` | JSON/table output |
| `src/cli/app/install.rs` | Best command example |
| `src/cli/context/create.rs` | Context create example |

## Quick Search

```bash
rg -n "#\[derive.*Parser\]" src/cli/
rg -n "#\[derive.*Subcommand\]" src/cli/
rg -n "pub fn " src/output.rs
rg -n "client\." src/cli/
```

## Usage Examples

```bash
meroctl --node node1 context ls
meroctl --node node1 context create --app-id <app-id>
meroctl --node node1 call <context-id> --method get --args '{"key":"test"}'
meroctl --node node1 blob upload --path ./file.txt
```

## Gotchas

- Always pass `--node <name>` before any subcommand
- App installation requires a context
