# Calimero v2 app runtime and SDK spec v0.14

Oct 27, 2026 · @Xabi

Changes in v0.14, from the G1 contract review:

- D49: range cursors read the state as of when they opened.
- D50: limits per run: 1,000 events (names up to 100 bytes, data up to 16 KiB), keys up to 1 KiB, 1,024 open cursors, 64 KiB of logs; the entry-size check covers the diff and the events together.
- D51: the ABI carries `abiVersion`; nodes ignore unknown keys and refuse bad values in known ones.
- D52: `unique-id` is `<leader term>.<call seq>.<n>`, amending D47; an add is logged as its summed value.
- The SDK env table now says migrations write only through `rebuild`.

> **Comment (Sandi, Oct 30), synchronous xcall:** proposal for review, not yet a decision. An app can call another app **in the same organization** and get its answer back in the same run: `xcall.call(app, method, args) -> result<list<u8>, string>`, plus a read-only `xcall.view`. The motivating case is a shared service, such as a captcha app that several apps in one org call to verify a token before they write. Async `xcall.send` (D25, D29, Q14) stays as it is.
>
> Why it fits this design: only the source's leader runs the source call, and followers apply its diff without re-running it. So the target's answer is just another input the leader reads, like the entry timestamp or the leader's CSPRNG. It reaches followers through the source's diff and is never replayed. No cross-group transaction is needed.
>
> The comments below mark each place in this spec the proposal touches. The node side (grants, the internal route that runs the target call, cycle and timeout handling, billing) is mine; the WIT, SDK and engine parts are yours.


Changes in v0.13, from the G0 spike and the scaling review:

- G0 passed: `fetch` over `wasi:http` works from a TypeScript action (Q5 closed, no fallback binding); a fresh instance costs 0.42 ms p50 for TypeScript and 0.011 ms for Rust, so D7 stays closed.
- D43: WASI pinned to 0.2.12, not 0.2.3, which no current toolchain builds.
- D44: nine WASI interfaces that toolchains import regardless of app code are accepted and served by host stubs that refuse; S1 changed to match.
- D45: Rust apps build for `wasm32-wasip2`; `mero build` calls componentize-js 0.23 directly, not through jco.
- Section 4 WIT fixed: it didn't parse (imported types clashed with the `caller` and `org-role` functions); the world now counts eight Calimero interfaces including `types`.
- D46: the host core reads state through a `StateView` trait, so a disk-backed store can replace the in-memory map later.
- D47: `storage.add` (commutative, no read) and `context.unique-id` keep table counts and generated ids from serializing every call.
- D48: `partition` reserved as a table attribute for later opt-in partitioning.
- Roadmap, not in this spec: disk-backed store, batched commits, pipelined execution, and parallel execution validated against log order; all invisible to apps.
- Stale text fixed: the prototype runtime version and D30's file name.

Changes in v0.12, from the developer agent's review of v0.11:

- D40: an action whose `ctx.call` gets `503 MIGRATING` ends its run without spending an attempt and re-runs from the start after the switch, instead of waiting inside its 10 s budget.
- D39: every rebuild attempt gets a fresh generation; the current generation is whichever the last switch named.

Changes in v0.11, from the developer agent's review of v0.10:

- D39: host-owned records (idempotency, timer slots, actions, xcall state) live outside the app's state keyspace; state keys are (app id, generation, table, …).
- D40: the action runner pauses during a rebuild; a sub-call that meets `503 MIGRATING` waits without spending an attempt.
- D41: the switch marks the old generation dropped; replicas reclaim it in the background.
- D42: during a rebuild each generation counts against the quota on its own.
- D29: time spent in a rebuild doesn't count toward the 1-hour xcall delivery window.
- S3 now refers to the D39 key layout.

Changes in v0.10, migrations become rebuilds:

- D38: a migration builds the new version in a separate keyspace from the untouched old state, then switches in one log entry. It replaces the in-place mechanics of D11 and D35.
- Views keep running on the old bundle and old state until the switch; a failed or abandoned rebuild leaves the old state byte-identical.
- No kept fields, no building indexes, no re-runnable transforms, no per-record cleanup: the old keyspace is dropped as one prefix.
- WIT world `calimero:app@0.5.0` adds a `rebuild` interface, usable only while handling `kind = migrate`.
- Read-time upgrades are optional again, for additive changes only.
- A9 updated; the developer's chunked-cleanup point from the v0.9 review no longer applies.

Changes in v0.9, from the developer agent's review of v0.8 (last change before G1):

- D34: a tick that traps or returns an AppError is logged with its status and counts as applied; a tick that times out is proposed again.
- D34: ticks pause during a migration and resume with one catch-up tick.
- D35: a migration transform keeps the fields it reads until the migration completes; a fixed bundle restarts the migration from the beginning; the harness runs every migration twice; owner docs state that a stuck migration blocks writes.

Changes in v0.8, from the developer agent's review of v0.7:

- D27: sub-call keys add an occurrence number, so an action can deliberately make the same call twice.
- D34: timer scheduling: who proposes ticks, how they are deduplicated, and what happens to missed ticks.
- D35: indexes and counts added or changed by a migration are marked building; views that query them get `503 MIGRATING`, and a unique clash stops the migration.
- D26: a vote request carries the candidate's signed Roughtime response; with no Roughtime the cluster takes no writes; merod-v2 measures time only with the TSC-backed monotonic clock.
- D36: a timeout returns `503 TIMEOUT` instead of `422 TRAP`.
- D37: TypeScript actions are static methods.
- Fixes: A6 accepts only `OutOfFuel`; CRM migration batches are 1,500; `calimero-client` is used by `calimero-cloud`, not `meroctl`; Q15 has a working answer.
- D30 now applies to v0.8: the baseline is committed as `docs/specs/app-runtime-v0.8.md`.

Changes in v0.7, decisions to start building:

- D30: this version is the baseline. The spec changes next only through the prototype report, or when implementation hits a blocker.
- D31: a one-day TypeScript spike (gate G0) runs before phase 1, to check `fetch` over `wasi:http`, the component's imports, size, start time and fuel.
- D32: the first milestone has a written definition of done, closes G1, and ends with a stop and a report.
- D33: checking the release-v2 review against the Architecture Specification is a separate track, not part of the prototype.
- Rollout and prototype sections updated to match. Q16 and Q17 don't block the first milestone; Q15 is still needed by G1.

Changes in v0.6, from the developer agent's third review:

- D26: Roughtime is the time bound against the operator, checked every 60 s, with the TD's TSC between checks and a defined failure action. The majority rule (D21) now only guards against one faulty host.
- D27: an action's sub-call keys include the method and an argument hash; a reused key with different arguments is refused.
- D28: the default fuel budget runs out well before 2 s at normal load, client retries of a Timeout are capped at 3, and unlogged timeouts count against the caller's quota.
- D29: a stuck xcall is retried in order for up to 1 hour, then dead-lettered; deleted targets dead-letter at once.
- Section 7 now says which code serves reads during a migration.

Changes in v0.5, from the developer agent's second review:

- Decisions D21–D25: time bounded by a majority of hosts, retries and a final failure for actions, timeouts that don't spend the idempotency key, signing-key lifecycle and third-party publishers, xcall deduplication by watermark.
- G2 no longer compares against wasmer, which can't run components; it compares two wasmtime configurations.
- Actions: wall clock rounded to the second, monotonic clock fixed (D19 now holds everywhere).
- The section 4 WASI table points to the section 5 time table; stale text fixed (six interfaces, runtime version, report version).

Changes in v0.4, from the developer agent's review:

- Decisions D15–D20: no legacy path for old apps, non-decreasing timestamps with index-based idempotency, durable actions run on the leader, bundles signed by a registered developer key, time and randomness in views, and number encoding across languages.
- WIT world: pinned WASI 0.2.3 versions, the `wasi:io` and `wasi:http/types` dependencies, an `action` interface; `HostCtx` is now generated from the WIT.
- Fixes: compiled apps load on every node; a failed call drops events and xcalls too; the monotonic clock no longer advances; actions run on the leader (S9 and the Actions paragraph agree); `Outcome` loses its `outbound` field.
- Sections are numbered, `minIndex` is defined, the rollout gates are listed as text, xcall is out of the prototype, and the parity checklist is attributed correctly.
- SDK reference tab: time, randomness, `startAction`, number encoding and migration reads updated.

Changes in v0.3:

- Six new decisions, D9–D14: indexes in the SDK, one storage encoding for both languages, chunked migrations, actions for outbound HTTP, an explicit caller for every invocation kind, and tenant isolation by plan.
- Outbound HTTP (`wasi:http`) is allowed only in actions, which run outside the log. Q2 is resolved.
- Correction: views run on every node, so every node loads the engine.
- New section: Prototype. Rooms and CRM, each built in Rust and in TypeScript, with acceptance criteria.

Changes in v0.2:

- WASI is no longer refused outright. The host implements a fixed set (clocks, random, stdio, environment, outgoing HTTP) with Calimero semantics; filesystem and sockets stay refused. Affects D6, section 4, section 5, section 6, S1 and Q5.
- Outbound HTTP uses the standard `wasi:http` interface; the custom `outbound` interface is removed.
- TypeScript apps can call `Date.now()` and `Math.random()`; the host serves the entry timestamp and the leader's randomness.

## 1. Purpose and scope

This spec defines how app code runs inside a v2 node: one host interface in WIT, one host core in Rust, and pluggable engines starting with wasmtime, so that Rust and TypeScript apps behave identically. It also turns the platform gaps found while writing five reference apps into prioritized requirements.

In scope:

- the executor boundary inside merod-v2, from an ordered call to the `Outcome` written to the log;
- the `calimero:app` WIT world, the host core, and engine adapters;
- limits, metering, isolation and error mapping;
- the Rust and TypeScript SDKs and their toolchains;
- platform requirements surfaced by the reference apps (Rooms, Notes, CRM, Records, Supplier portal).

Out of scope, covered by the Architecture Specification v0.1: Raft replication, identity, the sealed channel, attestation, keys, the control plane and releases. This spec only states what it needs from them.

Inputs: the Architecture Specification v0.1, `REVIEW-release-v2.md` (review of the release-v2 branches), and the five reference apps with their `GAPS.md`.

## 2. Decisions

| # | Decision | Why now | Revisit when |
| --- | --- | --- | --- |
| D1 | Replace wasmer with wasmtime | The host interface is being rewritten for v2 and has no external users; the apps in the apps repo are rebuilt on the new SDK (D15); after the SDK freeze a swap is a breaking release | Never for v2, unless a blocking runtime defect appears |
| D2 | Define the host interface once, in WIT (`calimero:app`) | One contract for every language and engine; WASI 0.3 made async native to components | — |
| D3 | The executor depends on an `Engine` trait, never on a runtime | Engines become replaceable and testable against each other | — |
| D4 | TypeScript is a first-class app language, beside Rust | Rust was the adoption bottleneck in both evaluation rounds; agents default to TypeScript | — |
| D5 | TypeScript runs as a WASM component (componentize-js on StarlingMonkey) inside wasmtime | One sandbox, one metering model, one trusted base | TS benchmarks show the interpreter is the bottleneck: then add a V8 adapter (D7) |
| D6 | Apps get WASI only as host-implemented interfaces with Calimero semantics; no filesystem or sockets | Standard toolchains (StarlingMonkey, Rust std) expect WASI; the host serves it from the log entry, the leader's randomness and declared hosts, never from the node's OS | — |
| D7 | V8 is deferred, not rejected | JIT speed, but a second sandbox, no instruction metering, and a frequent patch cadence inside a measured image | Evidence from D5's benchmarks |
| D8 | A fresh instance per call | No state can leak between callers through module or engine memory | Pooled instantiation cost exceeds the budget in section 5 |

Added in v0.3, to settle the questions that would otherwise force a rewrite during prototyping:

| # | Decision | Why | Revisit when |
| --- | --- | --- | --- |
| D9 | Tables, indexes, uniqueness and counts live in the SDK, on the host's ordered key-value storage | Calls run one at a time, so guest-side checks are safe; the host interface stays small and language-neutral | Benchmarks show guest-side indexes are too slow |
| D10 | One storage encoding for both SDKs: order-preserving tuple encoding for keys, deterministic CBOR for values. Calls stay `handle(method, args)` with JSON args per the ABI | Exports, admin tools, migrations and agent tools read any app's state whatever language wrote it; one interface for every app | — |
| D11 | Migrations run in chunks across log entries when they exceed one entry's limits; writes get `503 MIGRATING` meanwhile, views keep serving. Since v0.10 the chunks build a new keyspace instead of rewriting records in place (D38) | The spec's single-entry `migrate()` can't exceed 10,000 writes or 8 MiB | — |
| D12 | Outbound HTTP runs only in actions (`@app.action`): outside the log, 10 s budget, no direct state access; they write by calling normal methods, each its own entry | A call never waits on the network, so the log never stalls and the 2 s limit holds | — |
| D13 | Every invocation has an explicit caller: account or API key, `System` for timers and `migrate`, `App(source)` for delivered xcalls, the owner for `init`; actions pass on the caller who started them | Access checks must not quietly pass for timers or xcalls | — |
| D14 | Free, Basic and Premium apps share a process, isolated by the wasmtime sandbox, per-app encryption keys, a fresh instance per call and Spectre mitigations; Enterprise gets dedicated clusters | Matches the spec's plans; the executor keeps engines movable into worker processes later | A sandbox escape in wasmtime, or a customer requirement for process isolation |

Added in v0.4:

| # | Decision | Why |
| --- | --- | --- |
| D15 | No legacy path for v1-style core modules. Apps that prove the platform are rebuilt on the new SDK; the rest are archived. Apps on `SharedStorage` or `UserStorage` (mero-pass) move to `Roles` and `PerAccount` | A second host interface would undo D2 and bring back review issue 1 |
| D16 | Entry timestamps never decrease: `max(previous + 1 ms, leader clock)`. Idempotency retention is a number of log entries, not wall time | The review showed a skewed leader re-running acknowledged calls |
| D17 | Actions are durable: starting one commits `ActionStarted` with its key; the leader's action runner executes it in a worker pool outside the serial log and commits `ActionDone`; a new leader resumes pending actions with the same keys | At-least-once needs a record to retry from |
| D18 | The developer signs each bundle (component, ABI, manifest) with their own key, registered for the app at creation. Nodes verify every bundle against it; an upgrade needs the same signer plus the owner's passkey confirmation. Dev keys are refused in production | Method attributes and outbound hosts must come from the developer, not the operator |
| D19 | Views see the timestamp of the applied entry they read at, and the node's CSPRNG, unrecorded. Actions see the node's real wall clock rounded to the second, with the monotonic clock fixed at the action's start. Elsewhere the monotonic clock is fixed at the entry timestamp | Consistent with the data, and no timing side channel between tenants (D14) |
| D20 | Tables carry a run-time schema, generated from or checked against the ABI. Integer fields encode as CBOR integers and floats as CBOR floats in both languages; 64-bit integers are `bigint` in TypeScript and decimal strings in JSON arguments | TypeScript types disappear at run time, and JS numbers lose precision above 2^53 |

Added in v0.5:

| # | Decision | Why |
| --- | --- | --- |
| D21 | Heartbeat replies carry each follower's clock. The leader stamps entries with `max(previous + 1 ms, min(own clock, majority median + 2 s))`. A node whose clock is more than 2 s from the majority refuses to lead. Nodes check their clocks against several signed Roughtime servers at startup and periodically. This bounds D16 | Without a bound, one leader with a fast clock moves time forward for good; TDX has no trusted time source, so time is only as trustworthy as a majority of hosts |
| D22 | Action failures: a fault or out-of-fuel fails at once; host errors and timeouts retry with exponential backoff, 5 attempts by default, each logged; then a final `ActionFailed { reason }` entry and an `ActionFailed` event to the starter. An optional `onActionFailed` method can clean up. New leaders resume only actions without a final entry | Otherwise a failing action is resumed by every new leader forever |
| D23 | `Timeout` is not logged and doesn't spend the idempotency key; the client retries with backoff. Every other status stays logged | Timeouts depend on load, so replaying one forever would be wrong |
| D24 | The owner registers one or more signing keys per app, and rotates or revokes them with passkey confirmation. A lost key is replaced with confirmation, a 24-hour delay and notifications. A publisher can differ from the owner: an installed third-party app pins its publisher's key, and upgrades from that publisher need the org owner's approval unless the org opts into automatic updates | Completes D18 |
| D25 | xcall delivery is deduplicated with a per-source watermark: the target keeps the highest (source index, sequence) delivered from each source app | Delivery is ordered per source and target, so a watermark is enough, takes constant space, and needs no retention setting |

> **Comment (Sandi, Oct 30), synchronous xcall:** sync calls do not use the outbox, watermark or dead letters. They are deduplicated by an idempotency key derived from the source call's own key: `xc:<source app>:<source key>:<n>`, where `n` counts sync calls within the run. Suppose the source's leader fails after the target committed but before the source did. The source call is then retried with the same key on the new leader, which reaches the same `n`th call, and the target's idempotency record (D16) returns the stored answer instead of running twice. This needs the source call's key to be deterministic for timers too: the D34 (timer, slot) pair already is.


Added in v0.6:

| # | Decision | Why |
| --- | --- | --- |
| D26 | Roughtime bounds time against the operator. Each node queries at least 3 Roughtime servers run by independent operators and requires 2 to agree; it checks every 60 s and before it becomes leader. Between checks, time is the last verified Roughtime value plus elapsed time on the TD's TSC, read through the TSC-backed monotonic clock, never SystemTime. A vote request carries the candidate's latest signed Roughtime responses, and followers refuse to vote for a candidate whose responses are missing, stale or out of tolerance. A node whose check fails stops stamping entries: it steps down and serves views marked stale. If no node can reach Roughtime, the cluster accepts no writes until it can | The operator controls every host's clock, so a majority of hosts proves nothing against it; signed Roughtime responses can be delayed but not forged |
| D27 | An action's sub-call key is derived from the action key, the method, a hash of the arguments, and an occurrence number counting identical (method, argument hash) calls within the run. Every idempotency record stores the method and argument hash, and a reused key with different ones is refused with `409 IDEMPOTENCY_MISMATCH`, which only hand-written keys can trigger | A retried action that takes a different path must not get another call's cached result |
| D28 | The default fuel budget is sized so fuel runs out well before 2 s on a normally loaded node. Clients retry a `Timeout` at most 3 times. The gateway charges the CPU of unlogged timeouts to the caller's quota | Makes `Timeout` purely an overload signal, and stops a runaway call from being resubmitted forever for free |
| D29 | xcall delivery that hits `503 MIGRATING` or a timeout is retried with backoff, in order, for up to 1 hour; time while the target is rebuilding doesn't count toward the hour, and deliveries are held until the switch. Then the message goes to the source app's dead-letter list, the source's optional `onXcallFailed` method runs, and the watermark advances. An app error from the target is a final answer. When a target is deleted, its pending messages are dead-lettered at once and its watermarks are deleted | One stuck delivery must not block every later xcall from the same source |

Added in v0.7:

| # | Decision | Why |
| --- | --- | --- |
| D30 | From v0.7 on, the current version is the baseline for building, committed to the core repo as `docs/specs/app-runtime-v0.N.md`. No more review rounds before the prototype. The spec changes only through the prototype report, or when implementation hits a blocker, and each change bumps the version | The last review found four small issues, down from nine structural ones; the prototype will find more than further reading |
| D31 | Before phase 1, a one-day spike (G0) builds a minimal TypeScript action with componentize-js that calls `fetch` over `wasi:http`. It records the component's import list against the WIT world (section 4), the component size, the time to start a fresh instance, and the fuel used by one Rooms `book`. If the imports don't fit the world, or fresh instances are too slow, D7 and the instance lifecycle are revisited before phase 1 | TypeScript is the largest unknown left; finding it at G2 would cost the whole phase |
| D32 | The first milestone closes G1. Done means: the engine trait exists with wasmtime as the only engine, the WIT world and host core are reviewed by core leads, and Rooms in Rust passes A1, A3, A4 and A5 on the 3-node mock-TEE cluster. The developer then stops and writes a report: what differed from the spec, fuel and latency numbers, and proposed changes as the next spec version, without editing the spec directly | A fixed stop keeps scope from growing, and one report per milestone keeps the spec versioned |
| D33 | Checking the release-v2 review (REVIEW-release-v2) against the Architecture Specification runs as a separate task, in parallel or after, on its own branch | Keeps fixes to old code out of the prototype branch |

Added in v0.8:

| # | Decision | Why |
| --- | --- | --- |
| D34 | The app group's leader proposes timer ticks, stamped on the D26 clock. A tick is keyed (timer, scheduled time), with scheduled times aligned to the app's install time. The state machine stores the last scheduled time applied per timer and ignores any tick at or before it. A tick that traps or returns an AppError is logged with its status like any other call and counts as applied, so it isn't retried and the timer context's previous time stays correct; repeated failures show in the owner log. A tick that times out isn't logged (D23), so the leader proposes the same slot again. Ticks missed while there was no leader, or while a migration ran, run once, as one catch-up tick for the latest missed slot; the timer context gives the scheduled time and the previous one, so apps that need every slot can catch up themselves | A leader change must not run a tick twice, and an outage must not cause a burst of ticks |
| D35 | Superseded by D38 in v0.10. Was: in-place migration with building indexes, kept fields, re-runnable transforms and a final cleanup step | Index entries for unmigrated records are built from old fields, so queries on new indexes would silently return partial results |
| D36 | A call that hits the epoch deadline returns `503 TIMEOUT` with `retryAfter`, not `422 TRAP` | It is retryable and not logged (D23), so generic HTTP clients should treat it like the other retryable statuses |
| D37 | TypeScript actions are static methods; the build rejects an action that uses `this` | Actions have no state access; an instance method would make `this.table` reachable and trap only at run time |

Added in v0.10:

| # | Decision | Why |
| --- | --- | --- |
| D38 | A migration is a rebuild. The new bundle reads the old state and writes the new version, with all its indexes, counts and unique constraints, into a separate keyspace, in batches of one log entry each. Calls get `503 MIGRATING`, timers pause, actions wait (D40) and xcall deliveries are held (D29); views run on the old bundle against the old state. A unique clash or a trap stops the rebuild and reports it to the owner, who either deploys a fixed bundle, which discards the new keyspace and starts over, or abandons the upgrade and keeps running the old version. When the last batch commits, one `MigrationSwitch` entry makes the new bundle and keyspace current, and the old generation is marked dropped and reclaimed in the background (D41). Storage needs about twice the app's size during a rebuild (D42) | The old state is never modified, so rollback is free, views stay consistent, and the in-place rules (kept fields, building indexes, re-runnable transforms, chunked cleanup) aren't needed |

Added in v0.11:

| # | Decision | Why |
| --- | --- | --- |
| D39 | App data is keyed (app id, generation, table, index, key). Every rebuild attempt, including a restart with a fixed bundle, allocates a fresh generation that is never reused; the current generation is whichever the last switch named, so an attempt never writes into keys still being reclaimed from an abandoned one. Host-owned records are keyed (app id, host, kind, …), with no generation: idempotency records, each timer's last applied slot (D34), `ActionStarted` and `ActionDone` (D17), the xcall outbox, watermarks and dead letters (D25, D29), and migration progress. The guest can address only the current generation, through `storage`, and the attempt's generation during a rebuild, through `rebuild`; it can never address host records | A switch replaces the app's data; everything the platform needs to keep its promises across the switch must survive it |
| D40 | During a rebuild the action runner starts no new runs. A running action whose `ctx.call` gets `503 MIGRATING` ends its run at once, without spending a D22 attempt, and stays pending. After the switch or the abandonment it re-runs from the start with the same action key, on the bundle then current. Its sub-calls get the same D27 keys, so those already committed return their stored results, and outbound requests carry the same `Idempotency-Key`. An action whose method no longer exists ends with `ActionFailed` and reason `method removed` | An enrichment started just before a CRM rebuild must not fail because of it |
| D41 | The switch only marks the old generation dropped. Each replica reclaims its keys in the background, outside the apply path, and snapshots skip dropped generations | Deleting a whole app's keys while applying one entry would stall the group |
| D42 | During a rebuild each generation counts against the app's storage quota on its own, so having two copies can't fail the rebuild. The console shows the temporary storage, and billing charges it for the rebuild's duration | An app near its quota would otherwise fail halfway through every rebuild |

Added in v0.13:

| # | Decision | Why |
| --- | --- | --- |
| D43 | Every WASI interface in the world is pinned to 0.2.12, the version wasmtime 50 and componentize-js 0.23 (StarlingMonkey) ship. Components importing `wasi:http` at 0.2.10 link through semver matching | The 0.2.3 pin can't be built by any current toolchain (G0) |
| D44 | Nine interfaces are accepted at install and served by host stubs that refuse: `wasi:cli/{stdin, exit, terminal-input, terminal-output, terminal-stdin, terminal-stdout, terminal-stderr}` and `wasi:filesystem/{types, preopens}`. Preopens lists no directories, every filesystem function returns an error, stdin is empty, no terminal is present, and `exit` traps `Fault`. Nothing reaches the OS. `wasi:sockets` and any other interface stay refused | The Rust standard library on `wasm32-wasip2` and StarlingMonkey import these whatever the app does (G0); refusing them at install would refuse every app |
| D45 | Rust apps build for `wasm32-wasip2`. TypeScript apps build with componentize-js 0.23 called directly by `mero build`, never through `jco componentize`. The SDK hides the version specifier JavaScript needs on Calimero imports (`calimero:app/storage@0.5.0`) | `wasm32-unknown-unknown` can't build getrandom 0.3, so `std::time`, `uuid` and `chrono` would break; jco silently falls back to componentize-js 0.19.3, whose async exports trap (G0) |
| D46 | The host core reads app state through a `StateView` trait (get and range at an applied index), not a concrete map. The in-memory map is the first implementation; a disk-backed store with a snapshot per applied index replaces it without changing the engine, the WIT or any app | Keeping every app's state in memory limits how many tenants a node can host; the store must be swappable behind the contract |
| D47 | `storage.add(key, delta)` changes an integer without reading it; the value is a deterministic CBOR integer, an add counts as one write, and an add to a non-integer value traps `HostDenied`. `context.unique-id()` returns a unique id (format in D52), in calls, timers, init and xcall delivery only. The SDK keeps table counts and `Counter::add` with `add`, and generates ids with `unique-id`. `Counter::increment()` stays, documented as serializing every call that uses it | Calls that read and rewrite the same count or counter key conflict; without these, a later parallel executor would run CRM inserts one at a time |
| D48 | `partition = "<field>"` is reserved as a table attribute and refused by the build until it is supported. Apps that don't declare it keep one log with transactions across all their keys | Spreading one app across several logs can't be invisible; reserving the attribute keeps it opt-in later without breaking existing apps |

Added in v0.14:

| # | Decision | Why |
| --- | --- | --- |
| D49 | A range cursor sees committed state plus the run's writes made before it opened, never writes made after; reads and the diff use the newest writes | A scan that updates records so their index key moves forward would otherwise meet them again, and could loop until it runs out of fuel |
| D50 | Per run: at most 1,000 events, names up to 100 bytes, data up to 16 KiB; keys up to 1 KiB; 1,024 open cursors; 64 KiB of logs. Values are bounded only by the entry-size check, which covers the diff and the events together. `emit` returns fixed soft errors: too many events, name too long, data too large, malformed audience, unknown team, audience names another org | Rooms emits up to 500 events per tick; small keys keep the disk store and indexes cheap, and a cap is easy to raise later but breaks apps when lowered |
| D51 | The ABI carries `abiVersion` (1 today). Nodes refuse an unsupported version, ignore unknown keys and refuse bad values in known keys; a field nodes must enforce bumps the version | Refusing unknown keys would make every older node refuse apps from a newer SDK, even for optional metadata such as MCP tool descriptions |
| D52 | `unique-id` is `<leader term>.<call seq>.<n>`: the leader's term, its per-term execution counter and the position in the run. This replaces D47's log index and position. An add is recorded in the log as its summed value | The entry index isn't known while a call runs, so index-based ids could repeat across concurrent runs or a leader change; summed values keep the log format simple to replay and verify |

## 3. Runtime architecture

The executor sees only an `Outcome`; engines translate the WIT contract into calls on one host core, which alone touches state and policy.

> *Diagram (runtime layers: executor, engines, WIT contract, host core) is in the online doc; section 3 describes the same layers in text.*

A call goes down as an invocation plus a budget and comes back as an `Outcome`, which is exactly what the log stores. Followers apply diffs without re-executing calls, but every node loads the engine, because views and stale reads run on any node.

### Interfaces

```rust
/// Generated from the calimero:app WIT world (wasmtime::component::bindgen!), so it can't drift from it.
/// The shape, abbreviated:
pub trait HostCtx {
    // storage
    fn storage_get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, HostError>;
    fn storage_put(&mut self, key: &[u8], value: &[u8]) -> Result<(), HostError>; // recorded in the diff
    fn storage_delete(&mut self, key: &[u8]) -> Result<(), HostError>;
    fn storage_range(&mut self, start: &[u8], end: &[u8], reverse: bool) -> Result<Cursor, HostError>;
    // context
    fn caller(&self) -> Caller;                      // Account | ApiKey | Public | System | App
    fn app_id(&self) -> AppId;
    fn app_owner(&self) -> AccountId;
    fn org(&self) -> Option<OrgId>;
    fn org_role(&self, account: &AccountId) -> Option<OrgRole>;
    fn org_role_strict(&mut self, account: &AccountId) -> Option<OrgRole>;
    fn orgs_of(&self, account: &AccountId) -> Vec<OrgMembership>;
    fn in_team(&self, account: &AccountId, team: &TeamId) -> bool;
    fn applied_index(&self) -> u64;
    fn start_action(&mut self, method: &str, args: &[u8]) -> Result<ActionId, HostError>;
    // events, blobs, xcall, the action interface and the WASI implementations: section 4
}

/// One per engine. Knows nothing about Raft, identity or policy.
pub trait Engine: Send + Sync {
    fn prepare(&self, bundle: &Bundle) -> Result<Prepared, PrepareError>; // validate, compile, snapshot, cache
    fn invoke(&self, app: &Prepared, call: Invocation, host: &mut dyn HostCtx, budget: Budget) -> Outcome;
}

pub struct Budget { pub wall_clock: Duration, pub memory: u64, pub compute: ComputeLimit }
pub enum ComputeLimit { Fuel(u64), CpuTime(Duration) }

pub struct Outcome {
    pub status: Status,          // Ok | AppError { code, message } | Trap(TrapKind)
    pub result: Vec<u8>,         // JSON per the ABI
    pub diff: Vec<Write>,        // empty unless status is Ok
    pub events: Vec<Event>,      // empty unless status is Ok
    pub xcalls: Vec<Outbox>,     // empty unless status is Ok
    pub actions: Vec<ActionStart>, // ActionStarted records (D17); empty unless status is Ok
    pub usage: Usage,            // fuel and CPU time, for limits and billing
}
```

Rules:

- The host core is the only code that reads or writes state, applies access policy, or decides audiences.
- An engine adapter is generated bindings plus limit wiring. It holds no app state between calls.
- Any status other than `Ok` drops the diff, the events, the xcalls and any actions started. Except for a Timeout (D23), the call is still logged with its status and result, so its idempotency key is spent and a retry returns the same answer.

> **Comment (Sandi, Oct 30), synchronous xcall:** `Outcome` gains `sync_calls: Vec<SyncCall { app, method, target_index, status }>`. It is kept for audit and the app log only: followers never use it, because the answers are already folded into `diff`. **Not atomic:** the target commits its own entry before the source's call finishes. If the source then fails (`AppError`, a trap, or a lost leadership), the target's change stays. Callers must treat a sync call like an HTTP request to another service, not like a local function. For captcha that is the right behaviour: a used token stays used.


State access (D46): the host core reads app state through a `StateView` trait, `get` and `range` at an applied index, and never through a concrete map. Views get a view at the index they read; calls get the leader's latest. The in-memory map is the first implementation.

## 4. The calimero:app WIT world

Apps import eight Calimero interfaces (including types) and a fixed set of WASI interfaces that the host implements or stubs, and export one dispatcher; nothing else crosses the boundary.

```wit
package calimero:app@0.5.0;

interface types {
  type account-id = string;
  type org-id = string;
  type team-id = string;
  type blob-id = string;
  type action-id = string;
  record api-key-caller { key-id: string, account: account-id }
  variant caller { account(account-id), api-key(api-key-caller), public, system, app(string) }
  enum org-role { owner, admin, member, guest }
  record org-membership { org: org-id, role: org-role }
  variant audience {
    all, caller, accounts(list<account-id>), team(team-id),
    org-members(org-id), org-admins(org-id),
  }
  record event { name: string, data: list<u8>, audience: audience }
  record app-error { code: string, message: string }
  record blob-info { size: u64, sha256: list<u8>, mime: string }
  enum invocation-kind { call, view, action, timer, xcall-deliver, init, migrate }

> **Comment (Sandi, Oct 30), synchronous xcall:** add `xcall-call` (the target runs a mutating sync call) and reuse `view` for `xcall.view`. `xcall-call` gets the same caller (`App(source)`), time and randomness as `xcall-deliver`, but in the **target's** own entry. Each app's timestamps stay its own; the source's clock does not move.

}

interface storage {
  get: func(key: list<u8>) -> option<list<u8>>;
  put: func(key: list<u8>, value: list<u8>);
  delete: func(key: list<u8>);
  add: func(key: list<u8>, delta: s64);   // commutative, no read; value is a CBOR integer (D47)
  resource cursor { next: func(max: u32) -> list<tuple<list<u8>, list<u8>>>; }
  range: func(start: list<u8>, end: list<u8>, reverse: bool) -> cursor;
}

// Available only while handling kind = migrate (D38). `storage` reads the old state
// (writes to it trap HostDenied); `rebuild` reads and writes the new version's keyspace.
interface rebuild {
  use storage.{cursor};
  get: func(key: list<u8>) -> option<list<u8>>;
  put: func(key: list<u8>, value: list<u8>);
  delete: func(key: list<u8>);
  add: func(key: list<u8>, delta: s64);
  range: func(start: list<u8>, end: list<u8>, reverse: bool) -> cursor;
}

interface context {
  use types.{caller as caller-t, account-id, org-id, team-id, org-role as org-role-t, org-membership, action-id};
  caller: func() -> caller-t;
  app-id: func() -> string;
  app-owner: func() -> account-id;
  org: func() -> option<org-id>;
  org-role: func(account: account-id) -> option<org-role-t>;
  org-role-strict: func(account: account-id) -> option<org-role-t>; // read-index on the system group
  orgs-of: func(account: account-id) -> list<org-membership>;     // requirement R1
  in-team: func(account: account-id, team: team-id) -> bool;
  applied-index: func() -> u64;
  unique-id: func() -> string;   // <leader term>.<call seq>.<n> (D52); calls, timers, init, xcall delivery (D47)
  start-action: func(method: string, args: list<u8>) -> result<action-id, string>; // calls and timers (D17)
  // time and randomness: wasi:clocks and wasi:random, served by the host (D19)
}

interface events {
  use types.{event};
  emit: func(e: event) -> result<_, string>;
}

interface blobs {
  use types.{blob-id, blob-info};
  info: func(id: blob-id) -> option<blob-info>;
  grant: func(id: blob-id, ttl-seconds: u32) -> string;
}

interface xcall {
  send: func(app: string, method: string, args: list<u8>) -> result<_, string>; // outbox; delivered after commit
}

> **Comment (Sandi, Oct 30), synchronous xcall:** proposed additions:
>
> ```wit
> interface xcall {
>   send: func(app: string, method: string, args: list<u8>) -> result<_, string>;            // unchanged
>   call: func(app: string, method: string, args: list<u8>) -> result<list<u8>, xcall-error>; // target commits, then returns
>   view: func(app: string, method: string, args: list<u8>) -> result<list<u8>, xcall-error>; // read-only, at the target's applied index
> }
> variant xcall-error { denied(string), app-error(app-error), unavailable, timeout, cycle }
> ```
>
> `call` is allowed in `call`, `timer` and `xcall-call`; `view` is allowed in every kind except `migrate`. A target error is an answer, not a trap: the guest gets `Err` and decides. `denied` covers the cases where the node refuses: not the same org, not an `#[app::xcall]` method, no grant (see the callers table). `unavailable` covers a target that is migrating, deleted, or has no leader. `cycle` is A→…→A, refused before the target runs, with depth bounded by the node's existing `MAX_XCALL_DEPTH`.


// Available only while handling kind = action (D12, D17)
interface action {
  key: func() -> string;                                                    // this action run's idempotency key
  view: func(method: string, args: list<u8>) -> result<list<u8>, string>;
  call: func(method: string, args: list<u8>) -> result<list<u8>, string>;   // one log entry, key derived from key()
}

world app {
  import storage;
  import rebuild;
  import context;
  import events;
  import blobs;
  import xcall;
  import action;

  // WASI 0.2.12, what wasmtime 50 and componentize-js 0.23 ship (D43); implemented by the host (section 4 table).
  // The nine stubbed interfaces of D44 are accepted at install but are not part of the world.
  import wasi:io/error@0.2.12;
  import wasi:io/poll@0.2.12;
  import wasi:io/streams@0.2.12;
  import wasi:clocks/wall-clock@0.2.12;
  import wasi:clocks/monotonic-clock@0.2.12;
  import wasi:random/random@0.2.12;
  import wasi:cli/stdout@0.2.12;
  import wasi:cli/stderr@0.2.12;
  import wasi:cli/environment@0.2.12;
  import wasi:http/types@0.2.12;
  import wasi:http/outgoing-handler@0.2.12; // usable in actions only (D12)

  use types.{invocation-kind, app-error};
  export handle: func(kind: invocation-kind, method: string, args: list<u8>) -> result<list<u8>, app-error>;
}
```

Rules the host enforces, whatever the guest says:

- `handle` with `kind = view` runs with storage writes, `emit`, `xcall` and outgoing HTTP disabled; any attempt traps. A method marked `confirm` is refused as a view. This closes review issue 6.

> **Comment (Sandi, Oct 30), synchronous xcall:** carve out `xcall.view` from "`xcall` disabled": a view may call another app's view, because it writes nothing. `xcall.call` and `xcall.send` still trap `HostDenied` in a view.

- The method's attributes (`view`, `confirm`, `public`, `every`, outbound hosts) come from the bundle's ABI, signed with it. The host checks them before calling `handle`.
- Blob references in state are typed in the ABI, so the host tracks retention itself (requirement R8).
- A component that imports anything outside this world, including `wasi:filesystem` and `wasi:sockets`, is refused at install.

### WASI, implemented by the host

| WASI interface | What the app gets |
| --- | --- |
| `wasi:clocks/wall-clock` | Depends on the invocation kind: see Time and randomness in section 5 |
| `wasi:clocks/monotonic-clock` | Fixed for the whole invocation (section 5); no elapsed time is visible to guest code (D19) |
| `wasi:random/random` | Depends on the invocation kind: see Time and randomness in section 5 |
| `wasi:cli/stdout`, `stderr` | The owner-only app log (spec §21); never operator-visible |
| `wasi:cli/environment` | Empty: no variables, no arguments |
| `wasi:http/outgoing-handler` | Only inside actions: hosts declared in the signed manifest, 10 s budget, at-least-once; a call or view that tries it traps HostDenied |
| `wasi:filesystem`, `wasi:sockets` | `wasi:sockets`: refused at install. `wasi:filesystem/{types, preopens}`: accepted only as host stubs that refuse, with no directories (D44) |

Also imported, because the interfaces above depend on them: `wasi:io/streams`, `wasi:io/poll`, `wasi:io/error` and `wasi:http/types`, all pinned to 0.2.12 (D43). Move to WASI 0.3 once StarlingMonkey supports it; no one has checked 0.3 against this list yet.

Stubbed interfaces (D44), confirmed by G0: `wasi:cli/{stdin, exit, terminal-input, terminal-output, terminal-stdin, terminal-stdout, terminal-stderr}` and `wasi:filesystem/{types, preopens}`. Each is a host stub with the same rules, never the OS implementation: stdin is empty, no terminal is present, `exit` traps `Fault`, preopens lists no directories and every filesystem function returns an error.

Actions (D12): `handle` with `kind = action` runs on the leader, in a worker pool outside the serial log, with outgoing HTTP enabled and storage disabled. It reads through views and writes by invoking methods, each committed as its own entry with the action's caller. Actions are at-least-once; the SDK attaches an idempotency key to every method they invoke and to every outbound request.

Durability (D17): a client starts an action through the API, or a call or timer starts one with `start-action`. Either way an `ActionStarted` entry with the action's key commits first. The leader's action runner executes pending actions and commits `ActionDone`. After a leader change, the new leader resumes every pending action with the same key, so methods it already called aren't applied twice.

Failure (D22): a fault or out-of-fuel inside an action fails it at once. A host error or a timeout is retried with exponential backoff, 5 attempts by default, each attempt logged. After the last attempt the runner commits a final `ActionFailed { reason }` entry and sends an `ActionFailed` event to whoever started it. If the app declares `onActionFailed`, that method runs once, as a normal call with the action's caller, to clean up. New leaders resume only actions with no final entry.

Sub-call keys (D27): each method an action calls gets a key derived from the action key, the method, a hash of the arguments and an occurrence number for identical calls within the run. Calling the same method twice with the same arguments therefore makes two writes, a retry gets the same keys in the same order, and a retry that takes a different path gets new keys rather than another call's cached result.

Async plan:

- v0.2 SDKs are synchronous. Outbound HTTP goes through `wasi:http`, which is async in WASI 0.3, so the contract won't break when async guest bindings land.
- Rust calls it as an `async fn` through wit-bindgen. TypeScript's `fetch` maps onto `wasi:http` in StarlingMonkey, and G0 confirmed it works with componentize-js 0.23.
- Versioning: the package version is part of the ABI; a node supports the current and previous minor version.

## 5. Execution, limits and metering

Every call runs in a fresh wasmtime instance from a pooled, precompiled component, under four limits: fuel, wall clock, memory and stack. Gas totals will differ from v1, which is acceptable because in v2 only the leader executes; budgets and limits must match, not counts.

### Instance lifecycle

1. **Install:** validate the component against the world (section 4), compile with Cranelift, serialize the artifact, cache it on disk keyed by bundle hash, wasmtime version and engine config.
2. **Load:** deserialize into an `InstancePre` per app on every node, because views and stale reads run on any node. Artifacts are recompiled after a node release changes wasmtime.
3. **Call:** new `Store` from the pooling allocator, copy-on-write memory image, set fuel and the epoch deadline, call `handle`, collect the `Outcome`, drop the store.
4. **One `Engine` for all apps.** Unlike wasmer's metering middleware, wasmtime fuel doesn't require an engine per module.

### Limits

| Limit | v1 (wasmer) | v2 (wasmtime) |
| --- | --- | --- |
| Compute | Metering middleware, 1 point per operator, charged at branches and calls | `consume_fuel`, roughly one unit per instruction; per-call budget set on the store |
| Host function cost | Host writes the middleware's injected globals by name | Host calls `Store::set_fuel` / `get_fuel` through the store context; no injected globals |
| Wall clock | none | Epoch interruption: 2 s per call or view, 10 s per action; an epoch ticker thread per node |
| Memory | Custom tunables cap pages and table elements, set guard sizes | `StoreLimits` per store: memory pages, table elements, instances; pooling allocator slot sizes |
| Stack | 200 KiB | `max_wasm_stack` = 200 KiB |
| Features | threads off | threads, shared memory and atomics off; other proposals off unless a guest toolchain needs them |
| Module checks | no imported memory, memory export required, `_start` refused | component validated against the world; wasi:filesystem, wasi:sockets and unknown imports refused |

Billing and quotas use one unit for every engine: CPU time per call, measured by the host. Fuel stays as a hard per-call limit on WASM engines, so a runaway call stops at the same point regardless of node load.

### Time and randomness (D16, D19)

| Invocation | Wall clock | Monotonic clock | Randomness |
| --- | --- | --- | --- |
| Call, timer, init, migrate, xcall delivery | the entry timestamp | fixed at the entry timestamp | the leader's CSPRNG, reaching followers only through the diff |
| View | the timestamp of the applied entry the view reads at | fixed at that timestamp | the node's CSPRNG, not recorded |
| Action | the node's real clock, rounded to the second | fixed at the action's start | the node's CSPRNG, not recorded |

> **Comment (Sandi, Oct 30), synchronous xcall:** the target of `xcall.call` runs as a normal entry in its own log: its own entry timestamp and its own leader's CSPRNG, recorded through its own diff. The source records only the bytes it got back. The target of `xcall.view` reads at its leader's latest applied index, after a read-index round, so the source sees a linearizable answer.


The leader stamps each entry with `max(previous + 1 ms, min(own clock, majority median + 2 s))` (D21), so timestamps never go backwards and one host can push time forward by at most 2 s. Idempotency records are kept for a number of log entries per app, not a length of wall time; the default is set by benchmark.

The leader's own clock in that formula is the D26 clock: the last verified Roughtime value plus elapsed TSC time, never the host's clock directly.

Timers (D34): the leader proposes each tick keyed (timer, scheduled time) and stamped on the D26 clock. The state machine applies a tick only if its scheduled time is later than the last one applied for that timer, so a tick proposed by two leaders around a failover runs once. A tick that fails is logged with its status and counts as applied; a tick that times out is proposed again. Ticks missed during an outage or a migration collapse into one catch-up tick for the latest missed slot.

### Error mapping

Every engine maps its failures to one status set, written to the log. Any status other than Ok drops the diff, events, xcalls and started actions:

| Status | Cause | Diff |
| --- | --- | --- |
| `Ok` | method returned | applied |
| `AppError { code, message }` | method returned an error | empty |
| `Trap(OutOfFuel)` | fuel exhausted | empty |
| `Trap(Timeout)` | epoch deadline reached; not logged, retryable with the same key (D23); returned as 503 TIMEOUT (D36) | empty |
| `Trap(OutOfMemory)` | memory or table limit | empty |
| `Trap(StackOverflow)` | stack limit | empty |
| `Trap(Fault)` | out-of-bounds, misaligned, bad signature, arithmetic, unreachable, a JS exception | empty |
| `Trap(HostDenied)` | a forbidden host call, such as a write from a view | empty |

> **Comment (Sandi, Oct 30), synchronous xcall:** no new status. A sync call that fails comes back to the guest as `Err(xcall-error)`. Only the guest's own handling of it decides the outcome. Wall time spent waiting on the target counts toward the source's epoch deadline. The target gets what is left of the source's budget, capped by its own budget, so a chain can never outlive the first caller's `Trap(Timeout)`.


TypeScript traps carry a source-mapped stack in the operator-invisible app log (spec §21), never in the response.

### Parity checklist from the v1 runtime

From a code read of `b07423114` (master matches):

- Cranelift, not singlepass, is the v1 baseline; match its compile time (scaffolding-e2e compiles in about 1 s with compiler crates at `-O3`).
- wasmer is pinned to 6.1.0 because 7.x dropped compiler backends on Windows and its V8 backend can't take the metering middleware. That pin goes away with the switch.
- Host functions are bound over a raw-pointer `FunctionEnv` with panics caught. In v2 they are generated from WIT, and a host panic becomes `Trap(Fault)`.
- Guest memory is read through `MemoryView`. In v2 the component model's canonical ABI does the copying.
- `enable_perfmap` profiling has a wasmtime equivalent (`profiling_strategy`), behind the same feature flag.
- merod-v2 currently compiles each bundle once with `Engine::default()` and caches it in memory (`crates/node/src/app.rs:436-456`). Step 1 above replaces that with a disk cache that survives restarts.

## 6. Guest toolchains

One CLI builds both languages into the same artifact: a signed component plus its ABI, deployed the same way.

|  | Rust | TypeScript |
| --- | --- | --- |
| Bindings | `wit-bindgen` from `calimero:app` | componentize-js from `calimero:app`, typed `.d.ts` generated from the same WIT |
| Engine inside the component | none (compiled to WASM) | StarlingMonkey, pre-initialized snapshot |
| SDK package | `calimero-sdk` (Rust crate) | `@calimero/sdk` (npm) |
| App macros | `#[app::state]`, `#[app::call]`, `#[app::view]`, `#[app::every]`, `#[app::confirm]`, `#[app::public]` | decorators with the same names |
| Build | `cargo mero build` | `mero build` (esbuild bundle, then componentize-js 0.23 called directly, never through jco; D45) |
| Test harness | `calimero-sdk::testing` | `@calimero/sdk/testing` (section 10) |
| Outbound HTTP | in actions, wasi:http (async) | fetch in actions, over wasi:http (works, G0) |

### Rules for TypeScript apps

The build fails, rather than the app misbehaving at run time, when code breaks these:

1. Methods are synchronous; no `async`, `await` or Promises in app code (actions excepted: they may await fetch).
2. Pure JavaScript dependencies only. Imports of Node built-ins (`fs`, `net`, `http`, `child_process` and the rest) fail the build.
3. Bundle size limit: 4 MiB after minification (default, to confirm by benchmark).

`Date.now()` returns the entry timestamp and `Math.random()` draws from the leader's randomness, because the host serves `wasi:clocks` and `wasi:random`. `env.timeNow()` and `env.randomBytes()` remain as explicit equivalents.

### Manifest

```json
{
  "package": "com.example.rooms",
  "version": "1.0.0",
  "language": "typescript",
  "entry": "src/index.ts",
  "access": "org",
  "outbound": [],
  "releaseRange": ">=2.4 <3"
}
```

The platform, not the developer, picks the engine from `language`. `access` is `public`, `owner`, `org` or `org-and-partners` (R1).

### Client bindings

The ABI generates typed clients for TypeScript and Python: methods, views, events, error codes, and the `confirm` and `public` flags. Generated clients are the only supported way to call an app; they handle idempotency keys, `412 CONFIRM_REQUIRED` and event resumption.

`minIndex`: an option on views. The serving node waits until it has applied at least that log index before running the view, for up to 2 s, then answers `503`. Clients pass the highest index they have seen, which gives read-your-writes on any node.

## 7. SDK surface

The SDK adds tables with declared indexes and a small set of access helpers on top of the spec's plain collections, because every reference app rebuilt both by hand.

Full API for both languages, the client SDK and the test harness: SDK reference

### Collections (from the spec, §9.1)

`UnorderedMap`, `SortedMap`, `Vector`, `UnorderedSet`, `SortedSet`, `Counter`, and `FugueText` / `RichText` for collaborative text. All are plain data structures over the ordered key-value store.

In the Rust SDK an `UnorderedMap` reads in key order too (`page`, `range`, `prefix`, `first`, `last`), so it is also the `SortedMap`; `Set<T>` is both sets; `Cell<T>` holds one value, `Frozen<T>` content-addressed write-once values and `PerUser<T>` one value per caller account; `Text` / `RichText` are the Fugue texts. Their key layouts are listed in `crates/app-sdk/AGENTS.md` (encoding contract), which the TypeScript SDK must match.

### Tables with declared indexes (new, R2)

```ts
const contacts = new Table<Contact>("contacts", {
  key: "id",
  unique: { email: ["email"] },
  indexes: {
    byOwnerName: ["owner", "nameKey"],
    byCompany: ["companyId"],
  },
});

contacts.insert(c);                                   // maintains every index, refuses a duplicate email
contacts.update(id, patch);
contacts.find("byOwnerName", { owner: me }, { prefix: { nameKey: "ad" }, limit: 20, cursor });
contacts.count("byCompany", { companyId });           // maintained count, no scan
```

- Indexes and counts are maintained by the SDK inside the same call, so they can't drift.
- Queries are range and prefix reads over an index, with opaque cursors. No joins in v0.1.
- Full-text search is a separate, host-maintained index (spec §6.3 `search_index`), queried from views. It must be turned back on (review: disabled in release-v2).

### Access helpers (spec §12.4, extended)

| Helper | Covers |
| --- | --- |
| `requireMember()`, `requireAdmin({ strict })` | the guards every app wrote |
| `Access` = org, teams, accounts, private | per-object readers, with `canRead` and `audienceFor` |
| `Owned<T>` | owner-only edits |
| `Roles` | named app roles mapped to accounts |
| `PerAccount<T>` | one slot per caller |
| `notFoundUnlessVisible()` | hiding existence instead of returning Forbidden |

### Events

`env.emit(name, data, { audience })` with audiences `all`, `caller`, accounts, `team`, `orgMembers(org)`, `orgAdmins(org)`. The platform resolves team and org audiences at delivery time (R9), so a member who leaves stops receiving events immediately.

### Pagination

Every list view returns `{ items, next? }` with an opaque cursor. The SDK provides it; apps don't build sort keys by hand.

### Storage encoding (D10)

- Keys: an order-preserving tuple encoding, so `(owner, nameKey, id)` sorts and range-scans correctly whatever the element types. Tables prefix every key with the table name and index name.
- Values: deterministic CBOR (RFC 8949 core deterministic encoding). The Rust and TypeScript SDKs produce identical bytes for the same record.
- Calls: `handle(method, args)` with JSON arguments and results, described by the ABI.

### Callers (D13)

| Invocation | `env.caller()` | Default guard result |
| --- | --- | --- |
| Call or view | `Account` or `ApiKey` | as the app's checks decide |
| Public method | `Public` | refused unless the method is `public` |
| Action | the caller who started it | as for that caller |
| Methods invoked by an action | the same caller | as for that caller |
| Timer | `System` | refused unless the method allows `system` |
| Delivered xcall | `App(source)` | refused unless the method allows that source app |

> **Comment (Sandi, Oct 30), synchronous xcall:** same row for `xcall-call` and `xcall.view`. "The method allows that source app" becomes concrete:
>
> - the method is `#[app::xcall]`;
> - the source and the target are in the **same organization**, which is the only scope for sync calls (async keeps Q14's cross-org rule);
> - a method marked `#[app::xcall(granted)]` additionally needs a grant in system state, `GrantXcall { target, source, methods }`. The grant is added by a holder of a new org capability, `MANAGE_XCALLS`, and is shown in the app report and the console. Revoking it takes effect on the next call.
>
> The node checks all three before the target runs, so the target's code never hand-checks the caller. `from_same_app` keeps its meaning.

> **Comment (Sandi, Oct 30), synchronous xcall across machines:** the source's leader (node X) and the target's leader (node Y) are usually different machines in the same cluster. X sends the call to Y over the existing sealed, identified peer channel (`internal/call`, Noise IK with attested transport keys). Y runs the call and commits it with the target's replicas, then answers. The cost is one X→Y round-trip plus the target's commit; the same holds for `xcall.view` with a read-index round instead of the commit.
>
> Y must not take `App(source)` from the message. It sets the origin only if the channel's node X is the current leader, or at least a replica, of the source app in system state. Otherwise any attested node could claim to speak for any app. The peer release gate from Phase 0 applies too: the target's owner must have approved X's release. If Y is down, X retries against the target's new leader within the source's deadline using the same derived key, then returns `Err(unavailable)` or `Err(timeout)`. Calls across clusters are out of scope (Q14: same cluster only).

| `init` | the owner | allowed |
| `migrate` | `System` | allowed, migration context only |

### Migrations (D11, D38)

```ts
@app.migrate({ from: 1, to: 2, batch: 1_500 })
static migrateContacts(from: CrmV1, to: Crm, cursor?: Cursor): Cursor | "done" {
  // called repeatedly by the host, one log entry per batch, until it returns "done";
  // reads the old state through `from`, writes the new keyspace through `to`
}
```

A migration is a rebuild (D38). The new bundle reads the old state, which it can't modify, and writes the new version with all its indexes into a separate keyspace, one batch per log entry. Meanwhile calls return `503 MIGRATING` with a retry hint, timer ticks pause, running actions stop and re-run after the switch (D40), xcall deliveries are held (D29), views run on the old bundle against the old state, and the console shows progress. A trap or unique clash stops the rebuild: the owner deploys a fixed bundle, which discards the new keyspace and starts over, or abandons the upgrade and keeps the old version. When the last batch commits, one entry switches the app to the new bundle and keyspace; the old generation is marked dropped and reclaimed in the background (D41). Idempotency records, timer slots, pending actions and xcall state are host records outside the app's state keyspace, so they survive the switch (D39). Read-time upgrade helpers (`Table.upgrade(from, fn)`) remain for additive changes that need no rebuild.

## 8. Platform requirements from the reference apps

Nineteen requirements came out of writing Rooms, Notes, CRM, Records and Supplier portal; seven are P0 because no external app works well without them.

P0 = before the first external app. P1 = before general availability. P2 = after.

| ID | Requirement | Apps | Priority | Notes |
| --- | --- | --- | --- | --- |
| R2 | Tables with declared indexes, counts, full-text search | CRM, Notes, Records, Portal | P0 | Section 7. CRM kept 5 indexes by hand |
| R8 | Blob API and lifecycle: upload, typed `BlobRef` in state, grants, host-tracked retention | Records, Portal | P0 | Review: no public blob API in release-v2 |
| R9 | Team and org audiences, resolved at delivery | Notes, Portal | P0 | Section 7 |
| R17 | Test harness: accounts, orgs, time travel, timer ticks, confirm, event capture | all five | P0 | Section 10 |
| R18 | Shared helpers: guards, `Access`, pagination | all five | P0 | Section 7 |
| R4 | Service accounts: non-human org members with their own role and API keys | Portal, Records | P0 | An agent's access shouldn't depend on one employee |
| R3a | State on every agent surface that the agent's model sees what it reads | Portal | P0 | A sentence in docs and the MCP setup, until R3 exists |
| R1 | Cross-org apps: admit partner orgs, `orgs-of`, org audiences, `org-and-partners` access | Portal | P1 | The use case only Calimero offers. Both orgs on one cluster (accounts are per cluster) |
| R5 | MCP tools generated from views, read-only, with a matching API-key scope | Portal | P1 | Manifest `mcp` section |
| R6 | Audited reads: `@app.view({ audit: true })` appends an audit record without a full call | Records | P1 | Today an audited read is a call through the serial log |
| R7 | Erasure: per-subject keys, or a documented delay for log archive and snapshots | Records | P1 | Spec keeps archives 30 days under a retention lock |
| R10 | TypeScript client for collaborative text: local replica, op encoding, offline queue | Notes | P1 | Without it `FugueText` is unusable from a frontend |
| R15 | Per-account, per-method rate limits | Portal, Notes | P1 | Spec limits are per app and per IP |
| R16 | Tamper-evident audit export, signed by the enclave | Records | P1 | For regulators who don't trust the operator |
| R14 | Secrets for outbound calls, never visible in views, events or exports | Portal | P1 | Email provider keys and similar |
| R3 | Private inference in the enclave for agents | Portal | P2 | In the spec as "later" |
| R11 | Presence: ephemeral, unlogged pub/sub with the same audience checks | Notes | P2 | Cursors, typing |
| R12 | Idempotency retention chosen per app, beyond 24 h | Notes | P2 | Fugue ops are already idempotent by id |
| R13 | Batched jobs for bulk changes beyond 10,000 writes per call | CRM | P2 | Timer-driven queue pattern in the SDK |

## 9. Security requirements

The runtime must hold three properties: a call sees only its own app's state, nothing survives between calls, and every engine in the image is patched and measured like the rest of the node.

| ID | Requirement | Check |
| --- | --- | --- |
| S1 | No imports outside `calimero:app`, the host-implemented WASI set and the refusing stubs (section 4, D44); `wasi:sockets` and anything else are refused at install, and no stub reaches the OS | Install test with components importing wasi:sockets or an unlisted interface; conformance test that every stub refuses; conformance test that wall-clock returns the entry timestamp |
| S2 | Fresh store per call; no module memory, globals or engine heap carried between calls | Conformance test: a value written to a global in call 1 is absent in call 2 |
| S3 | App isolation in the host core: storage keys are prefixed by app id and generation in the host, never by the guest, and host-owned records sit in a separate host range the guest can't address (D39) | Cross-app read and write attempts trap `HostDenied` |
| S4 | No reserved key space reachable from guest storage calls | Fixes review issue 1 (private storage) by construction: there is no private space |
| S5 | Views are read-only and can't run `confirm` methods | Fixes review issue 6 |
| S6 | Host panics become `Trap(Fault)`, never a node crash; the node stays up and the entry is logged | Fuzz host functions with malformed guest input |
| S7 | Every engine is pinned per node release and inside the measured image; an engine security fix ships as a node release through the 2-of-3 approval | Release checklist |
| S8 | Precompiled artifacts are checked against bundle hash, engine version and config before loading; never loaded from outside the node's encrypted store | Tampered-artifact test |
| S9 | Outbound HTTP only from actions, which run on the leader, and only to hosts declared in the signed manifest; never from calls or views | Undeclared-host and view tests |
| S10 | App logs and traps never contain arguments, results or account ids outside the owner-only log route | `check-no-user-data-at-info` as a CI gate on the runtime crates |

Review findings that this spec closes by design: issue 1 (S3, S4), issue 6 (S5), issue 8 (writer rotation goes away with storage wrappers), issue 11 (xcall becomes an outbox in the WIT world, never silently dropped). The rest of the review's blockers sit outside the runtime and stay with the node work.

Tenant isolation (D14): on shared plans, apps of different customers run in one process inside one enclave, separated by the wasmtime sandbox, per-app encryption keys, a fresh instance per call and Spectre mitigations. A sandbox escape would expose other tenants on that node. The security docs say this plainly; Enterprise gets dedicated clusters in separate trust domains.

Bundle signing (D18): method attributes and outbound hosts are trusted only because the developer signed them. The node checks every bundle against the signer registered for that app before installing it.

Key lifecycle (D24): the owner registers, rotates and revokes signing keys with passkey confirmation; a lost key is replaced only after a 24-hour delay with notifications, which limits what a stolen owner account can do. Third-party apps pin their publisher's key, and the org owner approves each upgrade unless the org opted into automatic updates.

Time (D21, D26): against the operator, app time is within the Roughtime tolerance plus 2 s, checked every 60 s, with the TD's TSC measuring elapsed time between checks; merod-v2 reads only the TSC-backed monotonic clock. The majority rule only guards against a single faulty host, because the operator controls every host's clock. A node that fails its Roughtime check stops stamping entries, and followers won't elect it because vote requests carry signed Roughtime responses. Timers, expiries and leases are only as trustworthy as this bound. An operator who blocks Roughtime can stop writes; that is a denial of service the operator already has, not a new one. Working answer to Q15: the TDX module controls the TD's TSC and the host can't change its offset after the TD is built; the host-chosen TSC frequency is still to confirm in Intel's documentation.

Dependency: S3 and D14 rely on per-app storage keys. Today the cluster uses one key (review, area 6). Deriving per-app encryption keys belongs to the keys work under the Architecture Specification; this spec requires it before any shared-plan app holds real data.

## 10. Testing and conformance

An engine ships only when it passes the host conformance suite and matches the reference engine byte for byte on every reference app.

| Layer | What it proves | Gate |
| --- | --- | --- |
| Host conformance suite | Each WIT function behaves as specified: storage, caller, org, time, randomness, events, blobs, limits, traps, S1–S10 | Every engine, every PR touching the runtime |
| Differential tests | Same app, same inputs (timestamp and randomness supplied by the host) → identical result, diff, events and status on every engine | Required to add or replace an engine; during the switch: two wasmtime configurations (allocator, optimization level), and Rust against TypeScript builds (A2). wasmer can't run components, so it's not compared |
| Reference apps | Rooms, Notes, CRM, Records and Supplier portal, in Rust and TypeScript, with their test suites | Acceptance for SDK releases |
| Agent evaluation | The five-app evaluation rerun with Claude Code using only the public docs | Before each public SDK release; tracks time to working app and docs gaps |
| Benchmarks | Instantiation time, per-call latency, memory per hosted app, fuel and CPU per call, Rust vs TypeScript | Sets pooling sizes and decides D7 (V8) |

SDK test harness (R17), as used by the Rooms tests:

```ts
const t = TestApp.create(Rooms, { org: { admins: ["ada"], members: ["bob"] }, now: T0 });
const id = t.as("bob").call("book", room, T0, 2, "1:1");
expect(() => t.as("cy").call("book", room, T0, 1, "x")).toThrowAppError("Taken");
t.setTime(T0 + 16 * MIN); t.tick("releaseNoShows");
t.as("ada").confirmed().call("adminRelease", id, "reason");
expect(t.eventsFor("bob")).toContainEqual(expect.objectContaining({ name: "BookingReleased" }));
```

The harness runs the real component on the real engine with an in-memory host, so a passing test means the same thing on a node.

## 11. Rollout plan

Five phases, each closed by a gate; the SDK freezes last, after Rust and TypeScript both pass on wasmtime and agents can build working apps from the docs alone.

> *Diagram (rollout: 5 phases, gates G1 to G5) is in the online doc; the gates table below lists every gate, including G0.*

Phases 1–3 run beside the security fixes from the review, which don't touch the runtime. Phase 4 can start as soon as G2 passes, in parallel with phase 3. Owners and dates are open (section 13).

G0 runs before phase 1 (D31). Each gate ends with a stop and a report that proposes the next spec version (D30, D32). The check of the release-v2 review against the Architecture Specification is a separate track with its own branch (D33).

The gates, as text:

| Gate | Closes | Criterion |
| --- | --- | --- |
| G0 | TypeScript spike | fetch over wasi:http works in a componentize-js action, or the fallback in section 12 is chosen; import list, size, start time and fuel per Rooms book recorded (D31) |
| G1 | Contract | WIT world reviewed by core leads; the host core owns all state access; engine trait in place with wasmtime only; Rooms in Rust passes A1, A3, A4 and A5 on the 3-node cluster; milestone report delivered (D32) |
| G2 | wasmtime backend | Conformance suite green; differential test passes across two wasmtime configurations; limits parity with v1 checked by tests |
| G3 | TypeScript | All prototype apps pass in Rust and TypeScript; benchmarks recorded; D7 decided |
| G4 | P0 requirements | Agent evaluation rerun: working apps from the public docs alone |
| G5 | Freeze | SDK 1.0 published; breaking changes need a new world major |

## 12. Prototype

The prototype builds Rooms and CRM, each in Rust and in TypeScript, on the v0.13 runtime. It passes when all four builds meet the acceptance criteria below and the two builds of each app behave identically.

### Apps

| App | Exercises | Action added | Migration |
| --- | --- | --- | --- |
| Rooms | No double bookings, a timer (no-show release), step-up confirmation (admin release), views that hide titles, org roles, event audiences | `syncToCalendar`: posts a booking to a mock calendar host, then calls `markSynced` | none |
| CRM | A table with five indexes, unique emails, pagination, prefix search, maintained counts, bulk reassignment | `enrichCompany(domain)`: fetches a mock enrichment API, then calls `updateCompany` | v1 to v2: split `name` into `firstName` and `lastName`, chunked, over 50,000 contacts |

One ABI per app: the Rust and TypeScript builds of the same app expose identical methods, arguments, results, error codes and events. One generated client works against either build, and the test scenarios are shared.

### In scope

- Host core: storage overlay, context, events, the host-implemented WASI set, actions, the caller model (D13).
- wasmtime engine: pooling, fuel, epoch deadlines, limits, error mapping.
- Rust and TypeScript SDKs: tables (D9), shared encoding (D10), guards, `Access`, pagination, rebuild migrations (D11, D38), actions (D12).
- Test harness in both languages; `mero build` for both; generated TypeScript client.
- A local 3-node mock-TEE cluster with a mock HTTP server standing in for declared outbound hosts.

Out of scope: xcall, blobs, search, cross-org apps, team audiences, collaborative text, MCP tools, V8, real TDX hardware, billing.

G0 answered Q5: `fetch` over `wasi:http` works from a TypeScript action with componentize-js 0.23, so no temporary SDK binding is needed.

### Acceptance criteria

| ID | Criterion | How it's checked |
| --- | --- | --- |
| A1 | Each of the four builds passes its app's shared scenarios | Harness, then the 3-node cluster |
| A2 | Rust and TypeScript builds are equivalent: same seeded scenario gives identical results, error codes, events and exported state bytes | Scripted comparison; timestamps, randomness and callers fixed by the host. Raw diff order may differ |
| A3 | No double bookings and no duplicate emails under contention | 200 concurrent attempts from 3 nodes; every loser gets `Taken` or `Duplicate` |
| A4 | No acknowledged write is lost or applied twice when the leader dies under load | The review's fault test, run on both apps and both languages |
| A5 | Views on followers honour `minIndex` and hide what they should on every node | Rooms titles and other reps' CRM contacts checked from each node |
| A6 | Runaway code traps within budget and the node stays up; at normal load an infinite loop ends in OutOfFuel, never Timeout (D28) | Infinite loop at normal load: `OutOfFuel`; memory bomb: `OutOfMemory`; in both languages |
| A7 | Isolation rules hold | S1–S5 and S10: write from a view, confirm method as a view, socket or unlisted import refused at install, every stub refuses |
| A8 | Actions behave as specified | Declared host works; undeclared host refused; `fetch` in a call or view traps `HostDenied`; a retried action doesn't apply its writes twice |
| A9 | Chunked migration works | CRM v1 to v2 over 50,000 contacts: rebuilds in batches, calls get `503 MIGRATING` meanwhile, views keep serving the v1 state, the switch is one entry, final exported state identical in both languages. A rebuild stopped halfway and abandoned leaves the v1 exported state byte-identical |
| A10 | Callers are explicit | Timer runs as `System`; member guards refuse `System`; `init` sees the owner |
| A11 | An agent can extend an app from the docs alone | Claude Code adds recurring bookings to Rooms in TypeScript using only the prototype docs; recorded, not gating |

### Benchmarks to record

Not gating; they answer Q3, Q4 and Q6.

- Component size and compile time, per build.
- Instantiation time, p50 and p99, pooled with a fresh store, per language.
- Call latency, p50 and p99, on the 3-node cluster.
- Fuel and CPU time per call for `book`, `listContacts` and `searchContacts`, Rust against TypeScript.
- Memory per warm app, per language.
- Sustained writes per second on one app log.

### Build order

1. TypeScript spike, one day: closes G0 (D31).
2. Host core, wasmtime engine and the WIT world, with D9–D14 wired in.
3. Rust SDK; Rooms and CRM in Rust pass A1 in the harness.
4. TypeScript SDK and toolchain; Rooms and CRM in TypeScript pass A1, then A2.
5. Cluster runs: A3–A8 and A10.
6. Migration (A9), benchmarks, A11, report.

First milestone (D32): steps 2 and 3 for Rooms only, plus A1, A3, A4 and A5 for Rooms in Rust on the 3-node cluster. CRM and TypeScript wait. At the milestone the developer stops and delivers the report; work continues after it is reviewed.

### Deliverables

- A prototype branch per repo, split into reviewable PRs per crate.
- The four builds and the shared scenario scripts.
- A report: evidence for each criterion, the benchmark table, and the spec changes it found, as the next spec version.

## 13. Open questions

| # | Question | Needed by |
| --- | --- | --- |
| Q1 | Owners and target dates for each phase. Proposed: one owner for the runtime and host core, one for the SDKs and TypeScript toolchain | Phase 1 start |
| Q2 | Resolved in v0.3: outbound HTTP runs only in actions, outside the log, with a 10 s budget (D12). | WIT freeze (G1) |
| Q3 | Default fuel budget per call, and per plan, sized so fuel runs out well before 2 s on a normally loaded node (D28) | Phase 2 |
| Q4 | What benchmark result triggers a V8 adapter (D7): a per-call latency or CPU ratio against Rust? | G3 |
| Q5 | Answered by G0: fetch over wasi:http works with componentize-js 0.23; the import list is the world plus the D44 stubs | Phase 3 |
| Q6 | StarlingMonkey snapshot size and instantiation cost per call; does it fit the pooling budget for hundreds of apps per node? | Phase 3 benchmarks |
| Q7 | Cross-org apps need both orgs on one cluster while accounts are per cluster. Global identity first, or partner orgs limited to one region? | R1 design |
| Q8 | Erasure: per-subject keys (crypto-shred one person) or a documented delay for archives and snapshots? | R7 design |
| Q9 | Are views metered and billed like calls, or only rate-limited? | Pricing |
| Q10 | Full-text search: host-maintained per node from applied state (spec §6.3), or a replicated index? | R2 design |

Added in v0.4:

| # | Question | Needed by |
| --- | --- | --- |
| Q11 | How long do nodes support an old world major after a new one ships? | SDK 1.0 (G5) |
| Q12 | Default idempotency retention, in log entries per app (D16) | Phase 2 |
| Q13 | Which apps in the apps repo are rebuilt and which are archived (D15)? | Phase 3 |
| Q14 | xcall semantics for after the prototype: exactly-once into the target's log (deduplicated on source app, index and sequence), ordered per source and target, same cluster only, cross-org only when the target allows the source app. Agreed in v0.5, with deduplication by watermark (D25); implemented after the prototype | Before xcall leaves prototype scope |

> **Comment (Sandi, Oct 30), synchronous xcall:** propose adding **Q18: synchronous in-org xcall** (`xcall.call` / `xcall.view`), with these answers to confirm:
>
> 1. same organization only;
> 2. not atomic: the target commits first;
> 3. idempotent across a source failover through a derived key;
> 4. bounded by the source's deadline and `MAX_XCALL_DEPTH`;
> 5. cycles refused;
> 6. the target's CPU time is billed to the source's caller;
> 7. a source app runs one call at a time today, so a sync call holds only that app's queue, never the target's.
>
> Needed by: the captcha / shared-service apps. Open for you: the WIT shape above, and whether components get it before wasmer apps.


Added in v0.6:

| # | Question | Needed by |
| --- | --- | --- |
| Q15 | Does TDX protect the TD's TSC from the host (D26)? Working answer: yes, the TDX module controls it and the host can't change its offset after the TD is built. Still to confirm in Intel's TDX documentation: how the host-chosen TSC frequency is fixed, which the 60 s Roughtime re-check bounds anyway | G1 |
| Q16 | Which Roughtime servers, run by which independent operators, and what tolerance? | Phase 2 |
| Q17 | Default dead-letter retention, and whether developers can redrive dead-lettered xcalls (D29) | Before xcall leaves prototype scope |

---

# SDK reference

The API the prototype implements: the app SDKs for Rust and TypeScript, the client SDK, and the test harness. Names here are proposals for v0.14; the prototype report confirms or changes them.

## Packages and versioning

The v2 SDKs ship under new names, so the published v1 `calimero-sdk` crate stays untouched for v1 users (the release-v2 review found it had been changed in place).

| Package | Registry | For |
| --- | --- | --- |
| `calimero-app-sdk` | crates.io | Rust apps: macros, `env`, collections, tables |
| `calimero-app-sdk` with feature `testing` | crates.io (dev-dependency) | Rust test harness |
| `@calimero/sdk` | npm | TypeScript apps |
| `@calimero/sdk/testing` | npm (subpath) | TypeScript test harness |
| `cargo-mero`, `@calimero/cli` (`mero`) | crates.io, npm | build (Rust for `wasm32-wasip2`, TypeScript with componentize-js 0.23 called directly; D45), codegen, deploy, local cluster |
| `@calimero/client`, `@calimero/react` | npm | browser and Node clients, React hooks |
| `calimero-client` | crates.io | Rust client, used by the `calimero-cloud` CLI (`meroctl` stays v1 only) |
| `calimero-client-py` | PyPI | Python client |

Versioning rules:

- The SDK major version equals the `calimero:app` world major version. During the prototype both are 0.x; 1.0 ships at gate G5.
- Rust and TypeScript SDKs share version numbers: the same version exposes the same features.
- The ABI records the world version an app was built against. A node runs apps built for its current world minor and the previous one.
- A breaking change needs a new world major. Nodes support the old and new majors side by side for a deprecation window (open question).

## Defining an app

Both languages declare the same things with the same names; the build turns them into the same ABI.

| Concept | Rust | TypeScript | Options |
| --- | --- | --- | --- |
| State root | `#[app::state(version = 1)] struct Rooms { … }` | `@app.state({ version: 1 }) class Rooms { … }` | `version`: schema version, used by migrations |
| Methods block | `#[app::logic] impl Rooms { … }` | the class body | — |
| Initializer | `#[app::init] fn init() -> app::Result<Self>` | `@app.init static init(): Rooms` | runs once, caller = owner |
| Call (writes) | `pub fn m(&mut self, …)` | `@app.call m(…)` | — |
| View (reads) | `#[app::view] pub fn m(&self, …)` | `@app.view m(…)` | runs on any node |
| Action | `#[app::action] pub async fn m(ctx: &mut ActionCtx, …)` | `@app.action static async m(ctx: ActionCtx, …)` | outside the log; may fetch declared hosts |
| Timer | `#[app::every("1m")]` | `@app.every("1m")` | interval: `s`, `m`, `h`; caller = `System` |
| Step-up | `#[app::confirm]` | `@app.confirm` | fresh passkey assertion over the call |
| Public | `#[app::public(rate = "10/min")]` | `@app.public({ rate: "10/min" })` | callable without an account |
| Non-account callers | `#[app::allow(system)]`, `#[app::allow(app = "com.x.y")]` | `@app.allow({ system: true, apps: ["com.x.y"] })` | default: refused |
| Errors | `#[app::error] enum Error { Taken { start: u64 }, … }` | `app.errors({ Taken: { start: "u64" }, … })` | variant name = error code |
| Events | `#[app::event] enum Event { … }` | `app.events({ … })` | typed payloads in the ABI |
| Migration | `#[app::migrate(from = 1, to = 2, batch = 1500)]` | `@app.migrate({ from: 1, to: 2, batch: 1500 })` | chunked (D11) |

### The same method in both languages

Rooms' `book`, input validation left out:

```rust
use calimero_app_sdk::prelude::*;

#[app::state(version = 1)]
pub struct Rooms {
    rooms: Table<Room>,
    bookings: Table<Booking>,
    slots: UnorderedMap<(RoomId, u64), BookingId>,
    next_id: Counter,
}

#[app::error]
pub enum Error { LoginRequired, Forbidden, NotFound, Taken { start: u64 }, Invalid(String) }

#[app::event]
pub enum Event {
    SlotsTaken { room_id: RoomId, start: u64, end: u64 },
    Booked { id: BookingId, title: String },
}

#[app::logic]
impl Rooms {
    pub fn book(&mut self, room_id: RoomId, start: u64, slot_count: u32, title: String) -> app::Result<BookingId> {
        let me = guards::require_member()?;
        if !self.rooms.contains(&room_id)? { app::bail!(Error::NotFound); }
        let end = start + u64::from(slot_count) * SLOT_MS;
        for t in (start..end).step_by(SLOT_MS as usize) {
            if self.slots.contains(&(room_id.clone(), t))? { app::bail!(Error::Taken { start: t }); }
        }
        let id = BookingId::from(format!("b{}", self.next_id.increment()?));
        self.bookings.insert(Booking { id: id.clone(), room_id: room_id.clone(), start, end, holder: me.clone(), title: title.clone(), checked_in: false })?;
        for t in (start..end).step_by(SLOT_MS as usize) {
            self.slots.insert((room_id.clone(), t), id.clone())?;
        }
        env::emit(Event::SlotsTaken { room_id, start, end }, Audience::Org)?;
        env::emit(Event::Booked { id: id.clone(), title }, Audience::Accounts(vec![me]))?;
        Ok(id)
    }
}
```

```ts
import { app, env, guards, Counter, Table, UnorderedMap, Audience } from "@calimero/sdk";

export const Errors = app.errors({ LoginRequired: {}, Forbidden: {}, NotFound: {}, Taken: { start: "u64" }, Invalid: { reason: "string" } });
export const Events = app.events({
  SlotsTaken: { roomId: "string", start: "u64", end: "u64" },
  Booked: { id: "string", title: "string" },
});

@app.state({ version: 1 })
export class Rooms {
  rooms = new Table<Room>("rooms", { key: "id" });
  bookings = new Table<Booking>("bookings", { key: "id" });
  slots = new UnorderedMap<[RoomId, number], BookingId>("slots");
  nextId = new Counter("nextId");

  @app.call
  book(roomId: RoomId, start: number, slotCount: number, title: string): BookingId {
    const me = guards.requireMember();
    if (!this.rooms.has(roomId)) throw Errors.NotFound();
    const end = start + slotCount * SLOT_MS;
    for (let t = start; t < end; t += SLOT_MS) {
      if (this.slots.has([roomId, t])) throw Errors.Taken({ start: t });
    }
    const id = `b${this.nextId.increment()}`;
    this.bookings.insert({ id, roomId, start, end, holder: me, title, checkedIn: false });
    for (let t = start; t < end; t += SLOT_MS) this.slots.set([roomId, t], id);
    env.emit(Events.SlotsTaken({ roomId, start, end }), Audience.org());
    env.emit(Events.Booked({ id, title }), Audience.accounts([me]));
    return id;
  }
}
```

Both builds produce the same ABI: method `book(roomId, start, slotCount, title) -> string`, errors `Taken { start }` and the rest, and the same two events. Rust field names are converted to camelCase in the ABI.

## The env API

Every host capability, with where it may be called; calling one from a disallowed invocation kind traps `HostDenied`.

| Capability | Rust | TypeScript | Returns | Allowed in |
| --- | --- | --- | --- | --- |
| Caller | `env::caller()` | `env.caller()` | `Caller`: `Account`, `ApiKey`, `Public`, `System`, `App` | all |
| Caller's account | `env::caller().account()` | `env.caller().account()` | account id or none | all |
| App id, owner | `env::app_id()`, `env::app_owner()` | `env.appId()`, `env.appOwner()` | ids | all |
| Org | `env::org()` | `env.org()` | org id or none | all |
| Org role | `env::org_role(a)` | `env.orgRole(a)` | role or none, from the node's replica | all |
| Org role, linearizable | `env::org_role_strict(a)` | `env.orgRoleStrict(a)` | role or none, after a read-index round | all except `migrate` |
| Team membership | `env::in_team(a, t)` | `env.inTeam(a, t)` | bool | all |
| Time | `env::time_now()` or `std::time` | `env.timeNow()` or `Date.now()` | ms; the entry timestamp in calls, timers and migrations; the timestamp of the applied entry being read in views; the node's real clock, rounded to the second, in actions (D19) | all |
| Randomness | `env::random_bytes(n)` or `getrandom` | `env.randomBytes(n)` or `Math.random()` | in calls: the leader's CSPRNG, reaching followers only through the diff; in views and actions: the node's CSPRNG, not recorded | all |
| Applied index | `env::applied_index()` | `env.appliedIndex()` | u64 | all |
| Emit event | `env::emit(e, audience)` | `env.emit(e, audience)` | — | call, timer, init, xcall delivery |
| Cross-app call | `env::xcall(app, method, args)` | `env.xcall(app, method, args)` | —, delivered after commit | call, timer |

> **Comment (Sandi, Oct 30), synchronous xcall:** add two rows:
>
> | Capability | Rust | TypeScript | Returns | Allowed in |
> | --- | --- | --- | --- | --- |
> | Cross-app call, answered | `env::xcall_call(app, method, args)` | `await env.xcallCall(app, method, args)` | `Result<Vec<u8>, XcallError>`; the target has committed | call, timer, xcall-call |
> | Cross-app view | `env::xcall_view(app, method, args)` | `await env.xcallView(app, method, args)` | `Result<Vec<u8>, XcallError>` | all except migrate |
>
> Captcha, from the calling app: `let ok: bool = serde_json::from_slice(&env::xcall_call(&captcha, "verify", &args)?)?; if !ok { app::bail!("captcha failed") }`. On the captcha side: `#[app::xcall(granted)] pub fn verify(&mut self, token: String) -> app::Result<bool>`.

| App log | `env::log!(…)`, stdout | `console.log(…)` | owner-only log | all |
| State | collections and tables | collections and tables | — | read: all except action; write: call, timer, init, xcall delivery; migrate writes through `rebuild` only |

Starting an action from code: `env::start_action("enrich_company", args)` in Rust, `env.startAction("enrichCompany", args)` in TypeScript. Allowed in calls and timers; returns an action id. The `ActionStarted` record commits with the call, and is dropped if the call fails (D17).

Failed xcalls (D29): a delivery stuck for more than 1 hour, or addressed to a deleted app, goes to the source app's dead-letter list. Declare `#[app::on_xcall_failed]` in Rust or `@app.onXcallFailed` in TypeScript to react; it runs once with the target, method, arguments and reason, as `System`.

Actions get an `ActionCtx` instead of state:

| Capability | Rust | TypeScript |
| --- | --- | --- |
| Run a view | `ctx.view("method", args).await` | `await ctx.view("method", args)` |
| Invoke a method (one log entry) | `ctx.call("method", args).await` | `await ctx.call("method", args)` |
| Outbound HTTP | `ctx.fetch(request).await` (over `wasi:http`) | `await fetch(url, init)` |
| Idempotency key for this action run | `ctx.key()` | `ctx.key()` |

`ctx.call` attaches a key derived from `ctx.key()`, the method, a hash of its arguments and an occurrence number for identical calls in the run (D27). Two identical calls in one run make two writes; a retried action gets the same keys in the same order, so it never applies a write twice. The host refuses a reused key with different arguments with `409 IDEMPOTENCY_MISMATCH`, which only hand-written keys can trigger. During a migration, a `ctx.call` ends the run without spending an attempt; the action re-runs from the start after the switch, and its already committed sub-calls return their stored results (D40). Write actions so that a re-run from the start is safe: send `ctx.key()` as the idempotency key on outbound requests.

TypeScript actions are static methods (D37): they have no `this`, and the build rejects an action that uses it.

Failed actions (D22): declare a cleanup method with `#[app::on_action_failed]` in Rust or `@app.onActionFailed` in TypeScript. It runs once, as a normal call with the action's caller, and receives the action id, method and reason.

```ts
@app.onActionFailed
handleFailed(f: { actionId: string; method: string; reason: string }): void {
  if (f.method === "enrichCompany") this.markEnrichmentFailed(f.actionId, f.reason);
}
```

## Collections and tables

Tables are the default for records; collections remain for simple keyed state. Both store data in the shared encoding (D10), so a Rust build and a TypeScript build of the same app write identical bytes.

### Declaring a table

```rust
#[derive(Record)]
#[table(
    key = "id",
    unique(email = ["email"]),
    index(by_owner_name = ["owner", "name_key"], by_company = ["company_id"]),
    count(by_stage = ["stage"]),
)]
pub struct Contact { pub id: ContactId, pub name: String, pub name_key: String, pub email: Option<String>,
                     pub company_id: Option<CompanyId>, pub owner: AccountId, pub stage: Stage }
```

```ts
contacts = new Table<Contact>("contacts", {
  key: "id",
  unique: { email: ["email"] },
  indexes: { byOwnerName: ["owner", "nameKey"], byCompany: ["companyId"] },
  counts: { byStage: ["stage"] },
});
```

### Table methods

| Method | Rust | TypeScript | Notes |
| --- | --- | --- | --- |
| Insert | `insert(r)` | `insert(r)` | fails `Duplicate { index }` on a unique clash or an existing key |
| Insert or replace | `upsert(r)` | `upsert(r)` | unique checks still apply |
| Read by key | `get(&k)` | `get(k)` | record or none |
| Exists | `contains(&k)` | `has(k)` | — |
| Update | `update(&k, \|r\| …)` | `update(k, r => …)` | re-indexes changed fields |
| Delete | `remove(&k)` | `delete(k)` | removes index and count entries |
| Query an index | `find(Index::ByOwnerName, prefix, opts)` | `find("byOwnerName", prefix, opts)` | prefix = leading index fields; `opts`: `limit`, `cursor`, `reverse`, `from`, `to` |
| Count | `count(Count::ByStage, prefix)` | `count("byStage", prefix)` | maintained, no scan |
| Scan all | `scan(opts)` | `scan(opts)` | key order, paged |

A query returns `Page { items, next }`. `next` is an opaque cursor, valid on every node and tied to the index and the world version; `limit` is capped at 1,000.

Write cost: one insert writes 1 record + 1 entry per index, unique constraint and count. Bulk work must fit the 10,000-writes-per-call limit; use a timer-driven job for more (R13).

Contention (D47): maintained counts use the host's commutative `add`, so inserts into the same table don't conflict over a count key. For generated ids, use `env::unique_id()` in Rust or `env.uniqueId()` in TypeScript, built from the leader's term and its own call counter (D52), instead of `Counter::increment()`, which reads and rewrites one key and so serializes every call that uses it. Keep `increment` for when ids must be consecutive.

Partitioning (D48): `partition = "<field>"` is reserved on tables and refused by the build until it is supported. Without it, all of an app's keys share one log and every call is a transaction across all of them.

### Collections

| Type | Use for | Key operations |
| --- | --- | --- |
| `UnorderedMap<K, V>` | keyed values | `get`, `insert`/`set`, `remove`/`delete`, `contains`/`has` |
| `SortedMap<K, V>` | ordered keys, ranges | as above, plus `range`, `range_rev`/`rangeReverse`, `first`, `last` |
| `Vector<T>` | append-mostly lists | `push`, `get`, `set`, `len`, `iter` |
| `UnorderedSet<T>`, `SortedSet<T>` | membership | `insert`, `remove`, `contains`/`has` |
| `Counter` | ids, totals; `add` never conflicts, `increment` serializes (D47) | `increment`, `add`, `get` |
| `FugueText`, `RichText` | collaborative text | `apply(ops)`, `to_string`, `encode` |

### Encoding rules

- Key fields may be strings, integers, booleans, bytes or none; they sort in that type order, then by value.
- Values are deterministic CBOR with camelCase field names in both languages.
- No floating-point NaN; amounts of money are integers in minor units.
- Each table or collection is named explicitly (`"contacts"`), and the name is part of every key, so renaming a field in code never moves data.

### Numbers across languages (D20)

TypeScript types disappear at run time, so every table and collection value has a run-time schema. The SDK generates it from the ABI, or checks a hand-written one against it.

```ts
const Booking = t.record({
  id: t.string(), roomId: t.string(),
  start: t.u64(), end: t.u64(),        // bigint in TypeScript
  holder: t.accountId(), title: t.string(), checkedIn: t.bool(),
});
bookings = new Table("bookings", Booking, { key: "id" });
```

| ABI type | Rust | TypeScript | CBOR | JSON args and results |
| --- | --- | --- | --- | --- |
| `u8`–`u32`, `i8`–`i32` | integer types | `number` | integer | number |
| `u64`, `i64` | `u64`, `i64` | `bigint` | integer | decimal string |
| `f32`, `f64` | `f32`, `f64` | `number` | float, shortest exact form | number |
| `string`, `bool`, `bytes` | `String`, `bool`, `Vec<u8>` | `string`, `boolean`, `Uint8Array` | text, bool, bytes | string, bool, base64url |

A value that doesn't match its schema fails the call with `Trap(Fault)` before anything is written. Rooms' slot times are `u64`, so the TypeScript build uses `bigint` for them. The TypeScript examples in this reference use number for brevity; the prototype uses bigint.

## Errors

Apps raise typed errors that reach the client as a code plus fields; everything else is a trap, logged with an empty diff and reported without details.

| Source | Rust | TypeScript | Client receives |
| --- | --- | --- | --- |
| App error, declared | `app::bail!(Error::Taken { start })` | `throw Errors.Taken({ start })` | `422 APP_ERROR`, `code: "Taken"`, typed fields |
| Table constraint | `TableError::Duplicate { index }` | `TableError.Duplicate` | `422 APP_ERROR`, `code: "Duplicate"`, `index` |
| Guard refused | `guards::require_member()?` | `guards.requireMember()` | `422 APP_ERROR`, `LoginRequired` or `Forbidden` |
| Undeclared throw or panic | `panic!` | any other `throw` | `422 TRAP`, kind `Fault`; details only in the owner log |
| Limits | — | — | `422 TRAP`, kind `OutOfFuel`, `OutOfMemory` or `StackOverflow`; a timeout returns `503 TIMEOUT` with `retryAfter` (D36) |
| Forbidden host call | — | — | `422 TRAP`, kind `HostDenied` |
| Step-up needed | `#[app::confirm]` | `@app.confirm` | `412 CONFIRM_REQUIRED` with a challenge |
| Migration running | — | — | `503 MIGRATING` with `retryAfter` |

Rules:

- Declared error codes and their fields are part of the ABI; generated clients expose them as typed error classes.
- An `AppError` and a trap both return an empty diff, and the call is still logged, so its idempotency key is spent and a retry returns the same error.
- Error messages are visible to the caller. Never put another user's data in them; hide existence with `NotFound`, as the reference apps do.

Timeouts are the exception (D23, D36): a call that hits the epoch deadline is not logged, doesn't spend the idempotency key, and returns `503 TIMEOUT` with `retryAfter`, because it depends on load. The client retries it with the same key and backoff; every other status is final for that key.

Idempotency mismatch (D27): a key reused with a different method or arguments returns `409 IDEMPOTENCY_MISMATCH` instead of the earlier result. Generated clients never do this; it signals a bug in hand-written key handling.

## Actions, timers and migrations

The three invocation kinds that don't come straight from a user call, as the prototype apps use them.

### Action: CRM `enrichCompany`

Fetches company details from a declared host, then writes through a normal method. The manifest declares `"outbound": ["enrich.example.com"]`.

```rust
#[app::action]
pub async fn enrich_company(ctx: &mut ActionCtx, company_id: CompanyId) -> app::Result<()> {
    let company: Company = ctx.view("company", json!({ "id": company_id })).await?;
    let Some(domain) = company.domain else { app::bail!(Error::Invalid("no domain".into())) };
    let res = ctx.fetch(Request::get(format!("https://enrich.example.com/v1/{domain}"))
        .header("Idempotency-Key", ctx.key())).await?;
    let info: Enrichment = res.json()?;
    ctx.call("apply_enrichment", json!({ "id": company_id, "industry": info.industry, "size": info.size })).await?;
    Ok(())
}

#[app::allow(action)] // only callable from this app's actions, not directly by clients
pub fn apply_enrichment(&mut self, id: CompanyId, industry: String, size: u32) -> app::Result<()> { /* … */ }
```

```ts
@app.action
static async enrichCompany(ctx: ActionCtx, companyId: CompanyId): Promise<void> {
  const company = await ctx.view("company", { id: companyId });
  if (!company.domain) throw Errors.Invalid({ reason: "no domain" });
  const res = await fetch(`https://enrich.example.com/v1/${company.domain}`, {
    headers: { "Idempotency-Key": ctx.key() },
  });
  const info = await res.json();
  await ctx.call("applyEnrichment", { id: companyId, industry: info.industry, size: info.size });
}

@app.call @app.allow({ action: true })
applyEnrichment(id: CompanyId, industry: string, size: number): void { /* … */ }
```

### Timer: Rooms `releaseNoShows`

```rust
#[app::every("1m")]
pub fn release_no_shows(&mut self) -> app::Result<()> {
    let cutoff = env::time_now() - CHECK_IN_GRACE_MS;
    let due = self.bookings.find(Index::PendingByStart, (), FindOpts::to((false, cutoff)).limit(500))?;
    for b in due.items { self.release(&b, "no-show")?; }
    Ok(())
}
```

```ts
@app.every("1m")
releaseNoShows(): void {
  const cutoff = env.timeNow() - CHECK_IN_GRACE_MS;
  const due = this.bookings.find("pendingByStart", [], { to: [false, cutoff], limit: 500 });
  for (const b of due.items) this.release(b, "no-show");
}
```

Scheduling (D34): ticks fall on slots aligned to the app's install time, and each runs once even across a leader change. A tick that throws an app error or traps is logged and counts as applied, so it isn't retried; a tick that times out runs again for the same slot. Ticks pause during a migration. Ticks missed during an outage or a migration collapse into one catch-up tick for the latest slot. `env::timer()` in Rust and `env.timer()` in TypeScript give the tick's `scheduled` time and the `previous` one applied, so a timer that must cover every slot can loop over the gap.

The caller is `System`, so member guards would refuse it; timers call internal helpers directly, never guarded public methods. More than 500 due bookings carry over to the next tick.

### Migration: CRM v1 to v2

Splits `name` into `firstName` and `lastName`, in batches that each fit one log entry.

```rust
// The v1 state type stays in the bundle, read-only, for as long as v1 can be migrated from.
#[app::state(version = 1)] mod v1 { /* CRM v1 tables */ }

#[app::migrate(from = 1, to = 2, batch = 1500)]
pub fn split_names(from: &v1::Crm, to: &mut Crm, cursor: Option<Cursor>) -> app::Result<MigrateStep> {
    let page = from.contacts.scan(ScanOpts::after(cursor).limit(1500))?;
    for c in page.items {
        let (first_name, last_name) = split_name(&c.name);
        to.contacts.insert(Contact { id: c.id, first_name, last_name, email: c.email,
                                     company_id: c.company_id, owner: c.owner, stage: c.stage })?;
    }
    Ok(match page.next { Some(next) => MigrateStep::Continue(next), None => MigrateStep::Done })
}
```

```ts
@app.migrate({ from: 1, to: 2, batch: 1500 })
static splitNames(from: CrmV1, to: Crm, cursor?: Cursor): Cursor | "done" {
  const page = from.contacts.scan({ after: cursor, limit: 1500 });
  for (const c of page.items) {
    const { name, ...rest } = c;
    const [firstName, lastName] = splitName(name);
    to.contacts.insert({ ...rest, firstName, lastName });
  }
  return page.next ?? "done";
}
```

Each batch must stay within one entry's limits. Each contact update can rewrite the record plus up to five index entries, so 2,000 updates could reach about 12,000 writes, over the 10,000 limit; CRM therefore uses batches of 1,500. The harness reports the write count per batch to catch this before deploy.

Which code runs during a migration (D38): the old bundle keeps serving views against the old state until the switch. The new bundle runs only the migration: `from` reads the old state, and any write to it traps `HostDenied`; `to` is the new version's state in its own keyspace, with its indexes, counts and unique constraints maintained as records are inserted. When the migration returns done, one entry switches the app to the new bundle and keyspace, and the old one is reclaimed in the background (D41). Idempotency records, timer slots, pending actions and xcall state belong to the host, not to either keyspace, so they carry over unchanged (D39). Pending actions resume on the new bundle after the switch.

Failures (D38): a trap or a unique clash in `to` stops the migration and reports it to the owner, with the conflicting keys for a clash. The owner either deploys a fixed bundle, which discards the new keyspace and starts again from the beginning, or abandons the upgrade, which discards the new keyspace and resumes writes on the old version. The old state is never modified, so transforms don't need to be safe to re-run and no fields need to be kept.

Cost: storage holds both versions until the switch, about twice the app's size. In the harness, `run_migration()` / `runMigration()` runs the rebuild and switches; `abandon_migration()` / `abandonMigration()` checks that the old exported state is unchanged.

## Client SDK

Every client refuses to send anything until the node proves what it runs, and there is no setting, environment variable or default that turns that off. The release-v2 review found the browser client was the weak point, so the rules below are requirements, not guidance.

### Connect and attest

```ts
import { CalimeroClient } from "@calimero/client";

const client = await CalimeroClient.connect({
  cluster: "https://eu-1.calimero.cloud",
  minRelease: "2.4.0", // required; no default
});
```

`connect` fetches `/attestation` with a fresh nonce and checks, in order:

1. the DCAP signature chain and TCB status (collateral from Intel PCS when the node sends none);
2. the Sigstore-signed release list, against the signer identity pinned in the SDK;
3. MRTD and RTMR0–3 against one release at or above `minRelease`, refusing `debug` profiles;
4. the report data binding the transport key, app hash and nonce.

Only then does it open the sealed Noise channel. Any failure throws `AttestationError` and nothing is sent.

Mock quotes are accepted only with `dev: true` passed in code. The client then logs a warning on every call, and refuses to connect to any host that isn't `localhost`.

### Sign in

| Who | TypeScript | Python | Rust |
| --- | --- | --- | --- |
| Person, new | `client.auth.passkeyRegister({ name })` | — | — |
| Person | `client.auth.passkeyLogin()` | — | — |
| Agent or server | `client.auth.apiKey(key)` | `client.auth.api_key(key)` | `client.auth().api_key(key)` |
| Sign out | `client.auth.logout()` | `client.auth.logout()` | `client.auth().logout()` |

Sessions and refresh tokens are kept in memory only, never in `localStorage`. After a page reload the user signs in again with a passkey (conditional mediation keeps that to one tap). Persisting a session is an explicit opt-in.

### Calling an app

Bindings are generated from the app's ABI (`mero codegen --ts`, `--py`):

```ts
import { Rooms } from "./generated/rooms";

const rooms = Rooms.attach(client, APP_ID);
const id = await rooms.book(roomId, start, 2, "Design review");
const day = await rooms.availability(roomId, dayStart);           // reads your writes by default
const strong = await rooms.availability(roomId, dayStart, { consistency: "strong" });
await crm.enrichCompany(companyId);                                // actions are called the same way
```

- Every call gets an idempotency key automatically, reused on retry after `503`.
- The client tracks the last index it saw, and views pass it as `minIndex`, so a user always sees their own writes.
- Methods marked `confirm`: the client catches `412 CONFIRM_REQUIRED`, asks the passkey for an assertion over the challenge, and retries. An `onConfirm` hook lets the UI explain what is being approved first.

### Events

```ts
const sub = rooms.events.subscribe({
  SlotsTaken: e => render(e),
  BookingReleased: e => notify(e.reason),
}, { onResync: () => reload() }); // called on 410 GONE: the client fell too far behind
```

Subscriptions resume from the last index after a reconnect.

### Errors

| Class | When |
| --- | --- |
| `AppError` subclasses, one per declared code (`Rooms.Errors.Taken`) | the app refused the call |
| `TrapError` with `kind` | the call trapped |
| `ConfirmCancelled` | the user declined the passkey prompt |
| `UnavailableError` with `committed: false` or `"unknown"` | no leader; retry with the same idempotency key |
| `AttestationError` | the node failed verification; nothing was sent |

The client retries `503 TIMEOUT` automatically, with the same idempotency key and exponential backoff, at most 3 times (D28), before surfacing it as `UnavailableError` with `reason: "timeout"` and `committed: false`. Actions that end in `ActionFailed` surface as an `ActionFailed` event to the account that started them.

### Required tests for every client

The impostor suite from the review runs in each client's CI: a non-TEE endpoint, a wrong or revoked release, a debug profile, an unbound transport key, a replayed quote, and a relay that forwards a genuine quote but answers requests itself. All must be refused before any data leaves the device.

## Test harness

The harness runs the real component on the real engine with an in-memory host, so a passing test means the same on a node; the host supplies time, randomness and callers, which makes every run reproducible.

| Capability | Rust | TypeScript |
| --- | --- | --- |
| Create an app | `TestApp::<Rooms>::new(Org::new().admins(["ada"]).members(["bob"]), T0)` | `TestApp.create(Rooms, { org: { admins: ["ada"], members: ["bob"] }, now: T0 })` |
| Call as someone | `t.as_("bob").call("book", args)` | `t.as("bob").call("book", ...args)` |
| Anonymous, API key, system | `t.anonymous()`, `t.api_key("bot", scope)`, `t.system()` | `t.anonymous()`, `t.apiKey("bot", scope)`, `t.system()` |
| View | `t.as_("cy").view("availability", args)` | `t.as("cy").view("availability", ...args)` |
| Step-up confirmed | `t.as_("ada").confirmed().call(…)` | `t.as("ada").confirmed().call(…)` |
| Time | `t.set_time(ms)`, `t.advance(dur)` | `t.setTime(ms)`, `t.advance(ms)` |
| Randomness | `t.seed(n)` | `t.seed(n)` |
| Timers | `t.tick("release_no_shows")` | `t.tick("releaseNoShows")` |
| Actions | `t.as_("bob").action("enrich_company", args)` with `t.mock_http(host, handler)` | `t.as("bob").action("enrichCompany", ...args)` with `t.mockHttp(host, handler)` |
| Migrations | `t.upgrade_to(bundle_v2).run_migration()` | `t.upgradeTo(bundleV2).runMigration()` |
| Events | `t.events_for("bob")` | `t.eventsFor("bob")` |
| Assertions | `assert_app_error!(r, "Taken")`, `assert_trap!(r, OutOfFuel)` | `expect(fn).toThrowAppError("Taken")`, `toTrap("OutOfFuel")` |
| Write count per call | `t.last_usage().writes` | `t.lastUsage().writes` |
| State export | `t.export_state()` (bytes, D10) | `t.exportState()` |

Shared scenarios: each prototype app has one scenario file in a neutral format (JSON steps: caller, method, args, expected result or error). Both the Rust and TypeScript harnesses run it, and A2 compares their `export_state` bytes at the end.
