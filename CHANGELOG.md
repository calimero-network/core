# Changelog

## [Unreleased]

## [0.6.0] - 2025-05-5

- Added alias substitution and use command for streamlined context interactions
  ( [#1223](https://github.com/calimero-network/core/pull/1223) - thanks
  [@Nathy-bajo](https://github.com/Nathy-bajo),
  [#1171](https://github.com/calimero-network/core/pull/1171) - thanks
  [@rtb-12](https://github.com/rtb-12) )
- Added support for alias on context invitation and join command (
  [#1181](https://github.com/calimero-network/core/pull/1181) - thanks
  [@cy4n1d3-p1x3l](https://github.com/cy4n1d3-p1x3l),
  [#1151](https://github.com/calimero-network/core/pull/1151) - thanks
  [@iamgoeldhruv](https://github.com/iamgoeldhruv) )
- Introduced event-triggered command execution with context watch (
  [#1224](https://github.com/calimero-network/core/pull/1224) - thanks
  [@Nathy-bajo](https://github.com/Nathy-bajo) )
- Added support for no-auth mode for node (
  [#1174](https://github.com/calimero-network/core/pull/1174) )
- Enabled forced alias creation and validation for safer configuration (
  [#1227](https://github.com/calimero-network/core/pull/1227) - thanks
  [@rtb-12](https://github.com/rtb-12),
  [#1180](https://github.com/calimero-network/core/pull/1180) - thanks
  [@rtb-12](https://github.com/rtb-12) )
- Improved Dockerfile, login popup, and admin dashboard experience (
  [#1214](https://github.com/calimero-network/core/pull/1214),
  [#1209](https://github.com/calimero-network/core/pull/1209),
  [#1205](https://github.com/calimero-network/core/pull/1205) - thanks
  [@iamgoeldhruv](https://github.com/iamgoeldhruv) )
- Optimized CI/CD, e2e test reliability, and removed redundant config fields (
  [#1235](https://github.com/calimero-network/core/pull/1235),
  [#1218](https://github.com/calimero-network/core/pull/1218) - thanks
  [@Nathy-bajo](https://github.com/Nathy-bajo),
  [#1206](https://github.com/calimero-network/core/pull/1206) - thanks
  [@Nathy-bajo](https://github.com/Nathy-bajo) )

## [0.5.0] - 2025-03-27

- Added Ethereum integration
- Decoupled contracts from core repository
- Extended e2e tests to include proxy contract functionalities
- Added autonat protocol

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

- env::executor_id() for fetching the runtime identity (no arbitrary signing,
  however).
- env::context_id() for fetching the context ID.
- calimero_storage::collections::{Unordered{Map,Set},Vector} for conflict-free
  operations
- Self::external() for external (blockchain) operations

Node:

- Removed the coordinator
- All messages sent between peers are now end-to-end encrypted
- Peers can share the application blob between one another, in case one of them
  doesn't have it installed
- The node has been split up into 2 binaries
  - merod retains node-specific commands, init, run, config
  - meroctl hosts client commands like context create, etc..
- merod config now has a generic & more flexible interface
- query & mutate in the API have now been merged into just execute
- interactive CLI now uses clap, making it more robust (merod)
- Added --output-format json for machine-readable output (meroctl)

Integrations:

- NEAR: expanded implementation to include a deployment of a proxy contract for
  every created context, which facilitates context representation on the network
- Starknet: reached feature parity with the NEAR implementation, allowing
  contexts to be created in association with the Starknet protocol.

[unreleased]: https://github.com/calimero-network/core/compare/0.6.0...HEAD
[0.6.0]: https://github.com/calimero-network/core/compare/0.5.0...0.6.0
[0.5.0]: https://github.com/calimero-network/core/compare/0.4.0...0.5.0
[0.4.0]: https://github.com/calimero-network/core/compare/merod-0.3.1...0.4.0
[0.3.1]:
  https://github.com/calimero-network/core/compare/merod-0.3.0...merod-0.3.1
[0.3.0]:
  https://github.com/calimero-network/core/compare/merod-0.2.0...merod-0.3.0
[0.2.0]: https://github.com/calimero-network/core/releases/tag/merod-0.2.0
