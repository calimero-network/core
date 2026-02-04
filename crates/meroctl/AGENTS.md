# meroctl - CLI Tool

Command-line interface for managing Calimero nodes, apps, contexts, and blobs.

## Package Identity

- **Binary**: `meroctl`
- **Entry**: `src/main.rs`
- **Framework**: clap (CLI), tokio (async)

## Commands

```bash
# Build
cargo build -p meroctl

# Build release
cargo build -p meroctl --release

# Run
cargo run -p meroctl -- --node node1 context ls

# Test
cargo test -p meroctl
```

## CLI Structure

```
meroctl --node <name> <subcommand>
├── app           # Application management
│   ├── install   # Install app from URL/path
│   ├── list      # List installed apps
│   ├── get       # Get app details
│   └── uninstall # Remove app
├── context       # Context management
│   ├── create    # Create new context
│   ├── delete    # Delete context
│   ├── list      # List contexts
│   ├── get       # Get context details
│   ├── invite    # Invite member
│   ├── join      # Join via invitation
│   └── identity  # Identity management
├── blob          # Blob storage
│   ├── upload    # Upload blob
│   ├── download  # Download blob
│   ├── list      # List blobs
│   └── delete    # Delete blob
├── call          # Call context method
├── peers         # Peer management
└── node          # Node info
```

## File Organization

```
src/
├── main.rs              # Entry point
├── cli.rs               # Root clap command
├── cli/
│   ├── app.rs           # App subcommands parent
│   ├── app/
│   │   ├── install.rs   # Install command
│   │   ├── list.rs      # List command
│   │   ├── get.rs       # Get app command
│   │   ├── uninstall.rs # Uninstall command
│   │   ├── get_latest_version.rs  # Get latest version
│   │   ├── list_packages.rs  # List packages
│   │   ├── list_versions.rs  # List versions
│   │   └── watch.rs     # Watch command
│   ├── context.rs       # Context subcommands parent
│   ├── context/
│   │   ├── create.rs    # Create command
│   │   ├── delete.rs    # Delete command
│   │   ├── get.rs       # Get command
│   │   ├── list.rs      # List command
│   │   ├── invite.rs    # Invite command
│   │   ├── invite_by_open_invitation.rs  # Invite by open invitation
│   │   ├── invite_specialized_node.rs  # Invite specialized node
│   │   ├── join.rs      # Join command
│   │   ├── join_by_open_invitation.rs  # Join by open invitation
│   │   ├── update.rs    # Update command
│   │   ├── sync.rs      # Sync command
│   │   ├── watch.rs     # Watch command
│   │   ├── proposals.rs # Proposals command
│   │   ├── alias.rs     # Alias command
│   │   ├── identity.rs  # Identity subcommands parent
│   │   └── identity/
│   │       ├── generate.rs  # Generate identity
│   │       ├── grant.rs     # Grant capabilities
│   │       ├── revoke.rs    # Revoke capabilities
│   │       └── alias.rs     # Identity alias
│   ├── blob.rs          # Blob subcommands parent
│   ├── blob/
│   │   ├── upload.rs    # Upload blob
│   │   ├── download.rs  # Download blob
│   │   ├── list.rs      # List blobs
│   │   ├── delete.rs    # Delete blob
│   │   └── info.rs      # Blob info
│   ├── call.rs          # Call method command
│   ├── node.rs          # Node info command
│   ├── peers.rs         # Peer management
│   └── validation.rs    # Validation command
├── client.rs            # HTTP client wrapper
├── output.rs            # Output formatting (JSON/table)
├── output/
│   ├── common.rs        # Common output utilities
│   ├── applications.rs  # Application output
│   ├── contexts.rs      # Context output
│   ├── blobs.rs         # Blob output
│   ├── aliases.rs       # Alias output
│   └── proposals.rs     # Proposal output
├── common.rs            # Shared utilities
├── config.rs            # Configuration
├── connection.rs        # Connection handling
├── defaults.rs          # Default values
├── storage.rs           # Storage utilities
├── auth.rs              # Authentication handling
└── version.rs           # Version checking
```

## Patterns

### Subcommand Module Pattern

- ✅ DO: Parent file declares `mod` for children
- ✅ DO: Follow pattern in `src/cli/app.rs`

```rust
// src/cli/app.rs - Parent declares children
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

### Command Implementation Pattern

- ✅ DO: Follow pattern in `src/cli/app/install.rs`

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
        // Implementation
    }
}
```

### Output Formatting

- ✅ DO: Use `output.rs` for consistent formatting
- ✅ DO: Support both JSON and table output

## Key Files

| File                        | Purpose                       |
| --------------------------- | ----------------------------- |
| `src/main.rs`               | Entry point                   |
| `src/cli.rs`                | Root command, common args     |
| `src/client.rs`             | HTTP client wrapper           |
| `src/output.rs`             | JSON/table output             |
| `src/cli/app/install.rs`    | App install (good example)    |
| `src/cli/context/create.rs` | Context create (good example) |

## JIT Index

```bash
# Find all CLI commands
rg -n "#\[derive.*Parser\]" src/cli/

# Find subcommand enums
rg -n "#\[derive.*Subcommand\]" src/cli/

# Find output formatting
rg -n "pub fn " src/output.rs

# Find API calls
rg -n "client\." src/cli/
```

## Common Gotchas

- Always specify `--node <name>` before subcommand
- App installation requires context to deploy
- Use `--help` on any subcommand for options

## Usage Examples

```bash
# List contexts
meroctl --node node1 context ls

# Create context with app
meroctl --node node1 context create --app-id <app-id>

# Call a method
meroctl --node node1 call <context-id> --method get_value --args '{"key": "test"}'

# Upload blob
meroctl --node node1 blob upload --path ./file.txt
```
