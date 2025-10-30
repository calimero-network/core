# Changelog

## [Unreleased]

### Added

- **Automatic nested CRDT support** - Applications can now use natural nested structures without state divergence
  - `LwwRegister<T>` - Last-Write-Wins register for any value with timestamp-based conflict resolution
  - `Mergeable` trait - Universal merge interface for all CRDT types
  - Automatic merge code generation via `#[app::state]` macro
  - Global merge registry for runtime type dispatch
  - Runtime integration - WASM modules auto-register merge functions on load
  - Supports unlimited nesting depth: `Map<K, Map<K2, Map<K3, V>>>` works
  - Zero developer burden - no registration code, no merge calls needed
  - Backward compatible - existing apps work unchanged
  
### Fixed

- **RGA insert_str position bug** - Text was appending to end instead of inserting at specified position
  - Fixed tie-breaking logic in `get_ordered_chars()` to sort by descending timestamp
  - Ensures sequential mid-document insertions work correctly
  - Added regression test to prevent future breakage

### Documentation

- Added comprehensive nested CRDT documentation
  - User guide: `crates/storage/NESTED_CRDTS.md`
  - Architecture docs: `NESTED_CRDT_SOLUTION_COMPLETE.md`
  - Performance analysis: `WHEN_MERGE_IS_CALLED.md`
  - Implementation guides for future enhancements

## [0.8.0] - 2025-01-07

- Introduced comprehensive blob storage system with runtime API, peer-to-peer
  discovery, and CLI support. ([#1319], [#1337], [#1340], [#1342], [#1422],
  [#1361])
  - Blobs can be shared and discovered across peer nodes.
  - Full integration with `meroctl` and `merod` commands.
  - Support for blob deletion.
- Standalone authentication service with JWT-based authentication. ([#1336],
  [#1385], [#1470], [#1360])
  - Username/password authentication provider.
  - Support for multiple nodes from a single auth server.
  - WebSocket authentication support.
  - Mock JWT token generation endpoint for development.
- Automatic ABI emission from application code. ([#1392], [#1498], [#1415])
  - Semantic ABI emission with optimized type collection.
  - Released standalone ABI extraction tool (`mero-abi`).
- Private application data storage with `#[app::private]` macro. ([#1504])
  - Encrypted storage utilities for sensitive application data.
- `cargo-mero` CLI build tool for Calimero applications. ([#1317], [#1512])
- Migrated client functionality to separate crate for better modularity.
  ([#1432])
  - Python client bindings (moved to separate repository). ([#1436], [#1440])
- Implemented Prometheus metrics for network and context execution. ([#1429])
- Application watch command for monitoring app changes. ([#1476])
- Application uninstall command. ([#1408], [#1349])
- Implemented append-only log for state deltas. ([#1345])
- Stabilized delta sync mechanism. ([#1352], [#1389], [#1390])
- On-demand context sync. ([#1371])
- Decouple relayer as separate component. ([#1489])
- Decouple blockchain primitives from server. ([#1449])
- Deprecated Stellar integration. ([#1480])
- Workspace-wide version management. ([#1444])
- Bumped libp2p to latest version and Rust to 1.88.0. ([#1423])
- Server initialization improvements. ([#1305])
- Removed feature flags; enabled admin/jsonrpc/websocket unconditionally.
  ([#1522])
- Request/response debug logging in server. ([#1475])
- Added `is-authed` endpoint for authentication status. ([#1383])
- Added `NEAR_API_KEY` environment variable support. ([#1406])
- Made localhost:2528 default server if not specified. ([#1388])
- Removed interactive CLI from node crate. ([#1426])
- Pass aggregates by reference for improved WASM ABI compatibility. ([#1356])
- Fixed memory explosion in WASM execution. ([#1405])
- Fixed runtime pointer handling issues. ([#1459])
- Fixed forced init not removing old database. ([#1368])
- Improved auth header validation during JWT verification. ([#1471])
- Fixed broadcast-triggered sync exclusivity. ([#1390])
- Fixed server body truncation logging. ([#1510])
- Fixed application installation during initial context sync. ([#1344])
- Fixed context application updates. ([#1366])
- Major runtime logic module refactoring and documentation improvements.
  ([#1495], [#1497])
- Added comprehensive runtime unit tests for host functions. ([#1474])
- Multiple Docker setup fixes. ([#1359], [#1346])
- Build warning cleanup. ([#1514], [#1492], [#1519])

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

[unreleased]: https://github.com/calimero-network/core/compare/0.8.0...HEAD
[0.8.0]: https://github.com/calimero-network/core/compare/0.7.0...0.8.0
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
[#1294]: https://github.com/calimero-network/core/pull/1294
[#1295]: https://github.com/calimero-network/core/pull/1295
[#1296]: https://github.com/calimero-network/core/pull/1296
[#1297]: https://github.com/calimero-network/core/pull/1297
[#1300]: https://github.com/calimero-network/core/pull/1300
[#1302]: https://github.com/calimero-network/core/pull/1302
[#1303]: https://github.com/calimero-network/core/pull/1303
[#1305]: https://github.com/calimero-network/core/pull/1305
[#1317]: https://github.com/calimero-network/core/pull/1317
[#1319]: https://github.com/calimero-network/core/pull/1319
[#1336]: https://github.com/calimero-network/core/pull/1336
[#1337]: https://github.com/calimero-network/core/pull/1337
[#1338]: https://github.com/calimero-network/core/pull/1338
[#1340]: https://github.com/calimero-network/core/pull/1340
[#1342]: https://github.com/calimero-network/core/pull/1342
[#1344]: https://github.com/calimero-network/core/pull/1344
[#1345]: https://github.com/calimero-network/core/pull/1345
[#1346]: https://github.com/calimero-network/core/pull/1346
[#1349]: https://github.com/calimero-network/core/pull/1349
[#1352]: https://github.com/calimero-network/core/pull/1352
[#1354]: https://github.com/calimero-network/core/pull/1354
[#1355]: https://github.com/calimero-network/core/pull/1355
[#1356]: https://github.com/calimero-network/core/pull/1356
[#1357]: https://github.com/calimero-network/core/pull/1357
[#1358]: https://github.com/calimero-network/core/pull/1358
[#1359]: https://github.com/calimero-network/core/pull/1359
[#1360]: https://github.com/calimero-network/core/pull/1360
[#1361]: https://github.com/calimero-network/core/pull/1361
[#1366]: https://github.com/calimero-network/core/pull/1366
[#1367]: https://github.com/calimero-network/core/pull/1367
[#1368]: https://github.com/calimero-network/core/pull/1368
[#1369]: https://github.com/calimero-network/core/pull/1369
[#1370]: https://github.com/calimero-network/core/pull/1370
[#1371]: https://github.com/calimero-network/core/pull/1371
[#1374]: https://github.com/calimero-network/core/pull/1374
[#1375]: https://github.com/calimero-network/core/pull/1375
[#1376]: https://github.com/calimero-network/core/pull/1376
[#1377]: https://github.com/calimero-network/core/pull/1377
[#1378]: https://github.com/calimero-network/core/pull/1378
[#1381]: https://github.com/calimero-network/core/pull/1381
[#1382]: https://github.com/calimero-network/core/pull/1382
[#1383]: https://github.com/calimero-network/core/pull/1383
[#1384]: https://github.com/calimero-network/core/pull/1384
[#1385]: https://github.com/calimero-network/core/pull/1385
[#1387]: https://github.com/calimero-network/core/pull/1387
[#1388]: https://github.com/calimero-network/core/pull/1388
[#1389]: https://github.com/calimero-network/core/pull/1389
[#1390]: https://github.com/calimero-network/core/pull/1390
[#1392]: https://github.com/calimero-network/core/pull/1392
[#1395]: https://github.com/calimero-network/core/pull/1395
[#1398]: https://github.com/calimero-network/core/pull/1398
[#1399]: https://github.com/calimero-network/core/pull/1399
[#1400]: https://github.com/calimero-network/core/pull/1400
[#1402]: https://github.com/calimero-network/core/pull/1402
[#1403]: https://github.com/calimero-network/core/pull/1403
[#1405]: https://github.com/calimero-network/core/pull/1405
[#1406]: https://github.com/calimero-network/core/pull/1406
[#1408]: https://github.com/calimero-network/core/pull/1408
[#1410]: https://github.com/calimero-network/core/pull/1410
[#1412]: https://github.com/calimero-network/core/pull/1412
[#1413]: https://github.com/calimero-network/core/pull/1413
[#1415]: https://github.com/calimero-network/core/pull/1415
[#1417]: https://github.com/calimero-network/core/pull/1417
[#1418]: https://github.com/calimero-network/core/pull/1418
[#1419]: https://github.com/calimero-network/core/pull/1419
[#1422]: https://github.com/calimero-network/core/pull/1422
[#1423]: https://github.com/calimero-network/core/pull/1423
[#1426]: https://github.com/calimero-network/core/pull/1426
[#1428]: https://github.com/calimero-network/core/pull/1428
[#1429]: https://github.com/calimero-network/core/pull/1429
[#1430]: https://github.com/calimero-network/core/pull/1430
[#1431]: https://github.com/calimero-network/core/pull/1431
[#1432]: https://github.com/calimero-network/core/pull/1432
[#1436]: https://github.com/calimero-network/core/pull/1436
[#1440]: https://github.com/calimero-network/core/pull/1440
[#1444]: https://github.com/calimero-network/core/pull/1444
[#1449]: https://github.com/calimero-network/core/pull/1449
[#1450]: https://github.com/calimero-network/core/pull/1450
[#1451]: https://github.com/calimero-network/core/pull/1451
[#1452]: https://github.com/calimero-network/core/pull/1452
[#1453]: https://github.com/calimero-network/core/pull/1453
[#1454]: https://github.com/calimero-network/core/pull/1454
[#1456]: https://github.com/calimero-network/core/pull/1456
[#1459]: https://github.com/calimero-network/core/pull/1459
[#1460]: https://github.com/calimero-network/core/pull/1460
[#1461]: https://github.com/calimero-network/core/pull/1461
[#1463]: https://github.com/calimero-network/core/pull/1463
[#1465]: https://github.com/calimero-network/core/pull/1465
[#1470]: https://github.com/calimero-network/core/pull/1470
[#1471]: https://github.com/calimero-network/core/pull/1471
[#1474]: https://github.com/calimero-network/core/pull/1474
[#1475]: https://github.com/calimero-network/core/pull/1475
[#1476]: https://github.com/calimero-network/core/pull/1476
[#1477]: https://github.com/calimero-network/core/pull/1477
[#1479]: https://github.com/calimero-network/core/pull/1479
[#1480]: https://github.com/calimero-network/core/pull/1480
[#1481]: https://github.com/calimero-network/core/pull/1481
[#1485]: https://github.com/calimero-network/core/pull/1485
[#1486]: https://github.com/calimero-network/core/pull/1486
[#1488]: https://github.com/calimero-network/core/pull/1488
[#1489]: https://github.com/calimero-network/core/pull/1489
[#1490]: https://github.com/calimero-network/core/pull/1490
[#1491]: https://github.com/calimero-network/core/pull/1491
[#1492]: https://github.com/calimero-network/core/pull/1492
[#1495]: https://github.com/calimero-network/core/pull/1495
[#1497]: https://github.com/calimero-network/core/pull/1497
[#1498]: https://github.com/calimero-network/core/pull/1498
[#1499]: https://github.com/calimero-network/core/pull/1499
[#1500]: https://github.com/calimero-network/core/pull/1500
[#1503]: https://github.com/calimero-network/core/pull/1503
[#1504]: https://github.com/calimero-network/core/pull/1504
[#1505]: https://github.com/calimero-network/core/pull/1505
[#1510]: https://github.com/calimero-network/core/pull/1510
[#1511]: https://github.com/calimero-network/core/pull/1511
[#1512]: https://github.com/calimero-network/core/pull/1512
[#1514]: https://github.com/calimero-network/core/pull/1514
[#1516]: https://github.com/calimero-network/core/pull/1516
[#1517]: https://github.com/calimero-network/core/pull/1517
[#1518]: https://github.com/calimero-network/core/pull/1518
[#1519]: https://github.com/calimero-network/core/pull/1519
[#1520]: https://github.com/calimero-network/core/pull/1520
[#1521]: https://github.com/calimero-network/core/pull/1521
[#1522]: https://github.com/calimero-network/core/pull/1522
