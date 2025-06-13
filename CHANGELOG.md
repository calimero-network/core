# Changelog

## [Unreleased]

## [0.7.0] - 2025-06-13

- Massive rework of the core to the actor model. ([#1263], [#1132], [#1158],
  [#1232], [#1246], [#1238], [#1251])
  - The node can now handle requests to multiple contexts in parallel.
  - Node sync is now much more robust.
- Applications now compile once on first use, and are cached for subsequent
  invocations. ([#1291], [#1280]; thanks [@onyedikachi-david])
  - This leads to a x10±8 performance improvement in request execution.
- `meroctl` now supports remote node management. ([#1237]; thanks [@Nathy-bajo])
- Introduce alias listing to `meroctl`. ([#1276]; thanks [@cy4n1d3-p1x3l])
- Constrain `PrivateKey` exposure, protect it from being printed in logs, copied
  or sent over the wire. ([#1256]; thanks [@onyedikachi-david])
  - This also means context join no longer requires a private key, just the
    invitation payload.
- Introduce context config permission management to the API, web ui and CLI.
  ([#1233]; thanks [@onyedikachi-david], [#1240]; thanks [@Nathy-bajo])
- The CLIs now report when there is an available version update. ([#1226];
  thanks [@cy4n1d3-p1x3l])
- Introduce context proxy proposal management to the CLIs. ([#1285]; thanks
  [@rtb-12])
- `--version` output in `meroctl` and `merod` now includes some build info like
  git status and rustc version. ([#1257]; thanks [@dotandev])
- Nodes now advertise their public address, and TLS has been removed from the
  server. ([#1254])
- Replace all blocking operations with async equivalents. ([#1266]; thanks
  [@dotandev])
- Decouple rocksdb from calimero-store ([#1245]; thanks [@dotandev])
- Simplify `meroctl` connection handling significantly which makes it more
  robust and maintainable. ([#1293]; thanks [@Nathy-bajo])
- Fixed `context identity ls` crash when no default context is set. ([#1241])
- Remove only-peers, visited and gen-ext apps ([#1261], [#1270])
- Remove `node-ui` from the repo, fetching a pre-built release from it's own
  repository. ([#1268])
- Fix all docker image issues. ([#1294], [#1295], [#1296], [#1297])

## [0.6.0] - 2025-05-05

- Introduced default alias selection with the `use` command for contexts and
  identities. ([#1171]; thanks [@rtb-12])
- Introduced alias substitution in call arguments. ([#1223]; thanks
  [@Nathy-bajo])
- Support alias creation on context invitation and joining. ([#1181], [#1151];
  thanks [@cy4n1d3-p1x3l], [@iamgoeldhruv])
- Introduced event-triggered command execution with context watch. ([#1224];
  thanks [@Nathy-bajo])
- Permit running nodes without server authentication. ([#1174])
- Enabled forced alias creation and validation for safer configuration.
  ([#1227], [#1180]; thanks [@rtb-12])
- Introduced Dockerfile for meroctl. ([#1214])
- Improve the login experience in the webui. ([#1209])
- Added a way to launch the webui from the interactive CLI. ([#1205]; thanks
  [@iamgoeldhruv])
- Remove a redundant config field from the merod config. ([#1206]; thanks
  [@Nathy-bajo])

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

<!-- versions -->

[unreleased]: https://github.com/calimero-network/core/compare/0.7.0...HEAD
[0.7.0]: https://github.com/calimero-network/core/compare/0.6.0...0.7.0
[0.6.0]: https://github.com/calimero-network/core/compare/0.5.0...0.6.0
[0.5.0]: https://github.com/calimero-network/core/compare/0.4.0...0.5.0
[0.4.0]: https://github.com/calimero-network/core/compare/merod-0.3.1...0.4.0
[0.3.1]: https://github.com/calimero-network/core/compare/merod-0.3.0...merod-0.3.1
[0.3.0]: https://github.com/calimero-network/core/compare/merod-0.2.0...merod-0.3.0
[0.2.0]: https://github.com/calimero-network/core/releases/tag/merod-0.2.0

<!-- contributors -->

[@rtb-12]: https://github.com/rtb-12
[@cy4n1d3-p1x3l]: https://github.com/cy4n1d3-p1x3l
[@iamgoeldhruv]: https://github.com/iamgoeldhruv
[@Nathy-bajo]: https://github.com/Nathy-bajo
[@dotandev]: https://github.com/dotandev
[@onyedikachi-david]: https://github.com/onyedikachi-david

<!-- patches -->

[#1171]: https://github.com/calimero-network/core/pull/1171
[#1223]: https://github.com/calimero-network/core/pull/1223
[#1181]: https://github.com/calimero-network/core/pull/1181
[#1151]: https://github.com/calimero-network/core/pull/1151
[#1224]: https://github.com/calimero-network/core/pull/1224
[#1174]: https://github.com/calimero-network/core/pull/1174
[#1227]: https://github.com/calimero-network/core/pull/1227
[#1180]: https://github.com/calimero-network/core/pull/1180
[#1214]: https://github.com/calimero-network/core/pull/1214
[#1209]: https://github.com/calimero-network/core/pull/1209
[#1205]: https://github.com/calimero-network/core/pull/1205
[#1206]: https://github.com/calimero-network/core/pull/1206
[#1263]: https://github.com/calimero-network/core/pull/1263
[#1132]: https://github.com/calimero-network/core/pull/1132
[#1158]: https://github.com/calimero-network/core/pull/1158
[#1232]: https://github.com/calimero-network/core/pull/1232
[#1246]: https://github.com/calimero-network/core/pull/1246
[#1238]: https://github.com/calimero-network/core/pull/1238
[#1251]: https://github.com/calimero-network/core/pull/1251
[#1291]: https://github.com/calimero-network/core/pull/1291
[#1280]: https://github.com/calimero-network/core/pull/1280
[#1237]: https://github.com/calimero-network/core/pull/1237
[#1241]: https://github.com/calimero-network/core/pull/1241
[#1233]: https://github.com/calimero-network/core/pull/1233
[#1240]: https://github.com/calimero-network/core/pull/1240
[#1254]: https://github.com/calimero-network/core/pull/1254
[#1261]: https://github.com/calimero-network/core/pull/1261
[#1270]: https://github.com/calimero-network/core/pull/1270
[#1266]: https://github.com/calimero-network/core/pull/1266
[#1245]: https://github.com/calimero-network/core/pull/1245
[#1226]: https://github.com/calimero-network/core/pull/1226
[#1285]: https://github.com/calimero-network/core/pull/1285
[#1257]: https://github.com/calimero-network/core/pull/1257
[#1276]: https://github.com/calimero-network/core/pull/1276
[#1256]: https://github.com/calimero-network/core/pull/1256
[#1293]: https://github.com/calimero-network/core/pull/1293
[#1268]: https://github.com/calimero-network/core/pull/1268
