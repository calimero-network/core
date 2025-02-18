# Changelog

## [Unreleased]

## [0.4.0] - 2025-02-18

- Added Stellar integration
- Added support for aliases which can replace hash based IDs
- Minor fixes on admin dashboard
- Optimized e2e tests
- Optimized release process
- Unified release artifacts into single Github Release
- Extracted install scripts to `install-sh` repository

## [0.3.1] - 2025-01-29

- Fixed get application endpoint and the corresponding meroctl command

## [0.3.0] - 2025-01-16

- Introduced ICP integrations, achieving full feature parity with NEAR and
  Starknet
- Improved replay protection on external interactions, fixing spurious failures
  from expired requests
- Moved protocol selection to context creation, and out of the config
- Allowed the specification of all protocol's default context configuration
- Exposed, and enabled functionality for context proxy storage
- Introduced bootstrap command for quick development as a demo
- Added additional REST endpoints for easier access and information retrieval

## [0.2.0] - 2024-12-05

Rust SDK:

- env::executor_id() for fetching the runtime identity (no arbitrary signing,
  however).
- env::context_id() for fetching the context ID.
- calimero_storage::collections::{Unordered{Map,Set},Vector} for conflict-free
  operations
- Self::external() for external (blockchain) operations

Node:

- Removed the coordinator
- All messages sent between peers are now end-to-end encrypted
- Peers can share the application blob between one another, in case one of them
  doesn't have it installed
- The node has been split up into 2 binaries
  - merod retains node-specific commands, init, run, config
  - meroctl hosts client commands like context create, etc..
- merod config now has a generic & more flexible interface
- query & mutate in the API have now been merged into just execute
- interactive CLI now uses clap, making it more robust (merod)
- Added --output-format json for machine-readable output (meroctl)

Integrations:

- NEAR: expanded implementation to include a deployment of a proxy contract for
  every created context, which facilitates context representation on the network
- Starknet: reached feature parity with the NEAR implementation, allowing
  contexts to be created in association with the Starknet protocol.

[unreleased]: https://github.com/calimero-network/core/compare/0.4.0...HEAD
[0.4.0]: https://github.com/calimero-network/core/compare/merod-0.3.1...0.4.0
[0.3.1]:
  https://github.com/calimero-network/core/compare/merod-0.3.0...merod-0.3.1
[0.3.0]:
  https://github.com/calimero-network/core/compare/merod-0.2.0...merod-0.3.0
[0.2.0]: https://github.com/calimero-network/core/releases/tag/merod-0.2.0
