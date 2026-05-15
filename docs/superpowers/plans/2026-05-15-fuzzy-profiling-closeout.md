# Fuzzy-CI Profiling Closeout Plan

**Goal:** reliable CPU + memory flamegraphs across all 4 nodes of all 4 fuzzy
suites, for performance benchmarking.

**Scope:** finish PR #2342 plus four follow-up PRs. Each phase is
self-contained and can be executed in a separate session.

---

## Phase 0 — Current state (already shipped on PR #2342, origin tip `daed3a94`)

What works today:
- Harvest path fix (the PR's original purpose).
- Node-1 CPU flamegraphs render for all 4 suites (~1 MB SVG, real
  symbolized stacks via `perf record -g` + `libc6-dbg`).
- Memory flamegraphs for all 4 nodes of all 4 suites.
- `MEROBOX_STOP_TIMEOUT=120` (consumed by merobox v0.6.13+, ref
  `calimero-network/merobox#238`).
- `RUN echo "cache_bust=..." > /tmp/.profiling-cache-bust` — buildx
  cache-invalidation escape hatch (one-line bump if buildx serves a stale
  COPY layer again).
- Render-pipeline hardening in `scripts/profiling/generate-flamegraph.sh`:
  `pipefail`, persisted `*.perf-script.log` next to the SVG,
  error-encoded placeholder SVG (failure cause is visible in the SVG
  itself).
- 30 s SIGINT→SIGKILL grace for `perf record` in both
  `scripts/profiling/entrypoint-profiling.sh` (`stop_profiling()`) and
  `scripts/profiling/collect-from-containers.sh`.

What does NOT work today:
- Workers (nodes 2–4): `perf.data=0` (no file at all). Root cause now
  isolated — see Phase 2.
- `[unknown]` frame count per stack: 1389 – 1712. ~15 % reduction from the
  2021 baseline; further reduction needs `libgcc-s1-dbgsym` +
  `libstdc++6-dbgsym` from the ddebs archive — see Phase 3.

Anti-patterns confirmed dead (do not re-try without addressing first):
- `--call-graph dwarf,16384 -N` — on the post-#2351 merod, perf's
  internal `addr2line_configure` makes finalize ~6 × slower per MB; even
  180 s grace did not let finalize complete, leaving `perf.data` header
  `data size = 0`.
- `-m 16M` mmap ring — kernel `perf_event_mlock_kb` is 4 MB in the
  container; perf refused to start with `Permission error mapping pages`.
- `LABEL`-only cache-bust — buildx with `cache-from=type=gha` folds
  standalone LABELs into image metadata without a layer-hash boundary, so
  it does not invalidate the downstream COPY. Use `RUN` (as we do now).

---

## Phase 1 — Ship PR #2342 as-is

The harvest-path fix and the node-1 CPU flamegraph win are both real and
in shipping shape. Worker CPU profiling and `[unknown]` reduction are
strictly improvements that can land separately.

**Action items**
1. Wait for CI on origin tip `daed3a94` to clear (Release rebuilds the
   profiling image; fuzzy load test then consumes it).
2. Verify the latest fuzzy run's `profiling-reports-<suite>-N-<runid>`
   artifact contains `flamegraph-cpu-fuzzy-*-node-1.svg` ≥ 500 KB for
   every suite.
3. Merge with the existing PR description; the open follow-up issue
   (composite-action refactor, #2363) is already filed and referenced.

**Verification checklist**
- `gh run list --repo calimero-network/core --branch fix/fuzzy-profiling-harvest-path --workflow "Fuzzy Load Test - Long Running Performance" --limit 1`
  shows `success`.
- `gh api repos/calimero-network/core/actions/runs/<id>/artifacts --jq '.artifacts[] | select(.name|test("profiling-reports")) | "\(.size_in_bytes)\t\(.name)"'`
  shows every reports artifact ≥ 1 MB (memory-only is ~250 KB; the jump
  means CPU SVGs are present).
- `gh pr view 2342 --json mergeable,mergeStateStatus` reports
  `mergeable=MERGEABLE` and `mergeStateStatus=CLEAN`.

**Rollback plan**
- If the next CI run regresses node-1 CPU flamegraphs:
  1. `git revert <merge-commit-of-this-PR> -m 1` on master, OR
  2. Push a tag-only rollback Dockerfile bump (revert `cache_bust` and
     keep the rest) to force a clean image rebuild.

**Risk:** low. All the load-bearing changes already produced a working
CI run; this is a wait-and-merge.

---

## Phase 2 — Fix worker perf profiling (the user's primary ask)

### Root cause (confirmed from in-container logs of run 25922141312)

Both seed and workers boot with kernel `6.17.0-1010-azure`. Both fail
the first sanity check in
`scripts/profiling/install_kernel_tools()`:
```
WARNING: perf not found for kernel 6.17.0-1010
You may need to install the following packages for this specific kernel:
    linux-tools-6.17.0-1010-azure
    linux-cloud-tools-6.17.0-1010-azure
```

**Seed (node-1):** `apt-get install linux-tools-${kernel_version}` *succeeds* →
`perf is now working` → `perf record` starts normally.

**Workers (nodes 2–4):** same apt install *fails* silently
(stderr swallowed by `2>/dev/null`), then `linux-tools-generic` fallback
also fails, then `WARNING: CPU profiling unavailable.` →
`start_profiling()` returns at line 94 (`perf not compatible, skipping
CPU profiling`) → no `perf.data` ever written.

The Dockerfile's existing comment block (lines 19–25) already documented
this failure mode: *"apt-lock contention that starved 3/4 containers of
perf when they all booted simultaneously."* The pre-install of
`linux-tools-generic` (line 25) was meant to be a fallback that doesn't
require runtime apt; the runtime symlink path in the entrypoint to wire it
up does not work reliably on workers.

### Two complementary fixes (recommended: do both)

**Fix A — eliminate runtime apt dependence (Dockerfile)**

Make the symlink at *image build time* so the entrypoint's apt install is
never on the critical path.

File: `.github/workflows/deps/prebuilt.profiling.Dockerfile`

Below the existing `linux-tools-generic` install at line 25, find the
actual generic-perf binary path (`/usr/lib/linux-tools/*/perf`) and
symlink it into a stable location (e.g. `/usr/local/bin/perf-generic`) or
preemptively into the kernel-version path that the wrapper expects. The
latter requires knowing the kernel at build time, which we don't — so the
realistic fix is the entrypoint side (Fix B), with the Dockerfile pinning
a known-good generic-perf path.

**Fix B — make the entrypoint fallback robust (entrypoint script)**

File: `scripts/profiling/entrypoint-profiling.sh`

Current generic fallback (lines 56–76) silently breaks when
`/usr/lib/linux-tools/*/perf` glob expands to zero matches OR when the
symlink target ends up at the same kernel-version path as the failing one
(loop condition `[ "$(basename "$(dirname "$candidate")")" = "$kernel_version" ] && continue`
skips that case, which is the right behavior, but if there's literally only
one entry and it matches kernel_version, the loop exits with
`generic_perf=""`).

Three concrete improvements:

1. **Surface apt-install failures.** Today: `apt-get install -y -qq … 2>/dev/null`
   (line 45). Change to capture stderr to a log file and `cat` it on
   failure. So next time we see *why* the apt install failed on a worker
   (network ENOENT? lock? package not found?).
2. **Try a known absolute path for the generic perf** before the
   `for candidate in /usr/lib/linux-tools/*/perf` loop. The Dockerfile
   pre-installs `linux-tools-generic`, which on Ubuntu 24.04 lands the
   binary at a predictable path; pin it. If the pinned path exists, use
   it directly — no symlinking, no wrapper.
3. **If everything fails, emit a single hard-to-miss error** instead of
   the current scattered "WARNING" lines. Something like
   `[Profiling] ERROR: perf unavailable on $NODE_NAME — flamegraph will be
   missing. See <log path> for the apt failures.`

### Verification

After the change, on a fresh CI run:
1. `grep -c "perf is now working\|perf working" <each-node-log>` → ≥ 1
   per node.
2. `grep "perf not compatible, skipping CPU profiling" <each-node-log>` →
   0 per node.
3. Per-node harvest line in the workflow log:
   `<node>: <size> (perf.data=1, heap=N)` for every node (was
   `perf.data=0` on workers).
4. `profiling-reports-<suite>-*` artifact contains
   `flamegraph-cpu-fuzzy-<suite>-node-{1,2,3,4}.svg` ≥ 500 KB for every
   node of every suite.

### Anti-patterns to avoid

- Don't add runtime `apt-get update` retries — apt-lock contention on
  Azure runners is a known flake, the right fix is to not depend on
  runtime apt.
- Don't blindly install `linux-tools-${kernel_version}` at *image-build*
  time — the runner kernel can change between image build and image use,
  and pinning to a kernel-specific package would break on future runner
  upgrades. Use `linux-tools-generic` (already installed) + a robust
  fallback.
- Don't silently swallow stderr from apt or perf commands going forward —
  that's the reason this took so long to diagnose. Capture to a log file
  and surface on failure.

### Rollback

- Single file change in each path. `git revert <commit>` on master.

### Risk: medium

The fix is in image-build / entrypoint-script logic, not perf record
flags, so risk to working node-1 profiling is low. There is non-zero risk
that the apt-install failure on workers has a different root cause we
haven't seen (e.g. a transient registry issue specific to that CI run);
the improved diagnostics in Fix B step (1) will tell us if so.

---

## Phase 3 — Reduce `[unknown]` frame count via ddebs

### Documentation reference (verified)

- Ubuntu wiki canonical setup:
  `https://wiki.ubuntu.com/Debug%20Symbol%20Packages` (the
  "Getting -dbgsym.ddeb packages" section)
- Live `Packages.gz` for noble:
  `http://ddebs.ubuntu.com/dists/noble/main/binary-amd64/Packages.gz`
- Both `libgcc-s1-dbgsym` and `libstdc++6-dbgsym` confirmed to exist on
  noble for both amd64 and arm64; the noble-updates / noble-security
  pocket carries the newer `14.2.0-4ubuntu2~24.04.1` build, which is what
  a freshly-updated `ubuntu:24.04` image will pin to.

### What to add

File: `.github/workflows/deps/prebuilt.profiling.Dockerfile`

Add a separate `RUN` block before the existing `apt-get install` at line
16 (mixing into the existing RUN would require two `apt-get update` calls
inside one layer):

```dockerfile
# Enable Ubuntu ddebs archive and install -dbgsym packages for libgcc/
# libstdc++ so perf can resolve frames in the C++ runtime and the
# unwinder helpers. Cuts the [unknown] frame count by ~20–30 %.
# ubuntu-dbgsym-keyring provides
#   /usr/share/keyrings/ubuntu-dbgsym-keyring.gpg
# (see packages.ubuntu.com filelist for noble).
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        ubuntu-dbgsym-keyring \
    && . /etc/os-release \
    && printf '%s\n' \
        "deb [signed-by=/usr/share/keyrings/ubuntu-dbgsym-keyring.gpg] http://ddebs.ubuntu.com ${VERSION_CODENAME} main restricted universe multiverse" \
        "deb [signed-by=/usr/share/keyrings/ubuntu-dbgsym-keyring.gpg] http://ddebs.ubuntu.com ${VERSION_CODENAME}-updates main restricted universe multiverse" \
        "deb [signed-by=/usr/share/keyrings/ubuntu-dbgsym-keyring.gpg] http://ddebs.ubuntu.com ${VERSION_CODENAME}-proposed main restricted universe multiverse" \
        > /etc/apt/sources.list.d/ddebs.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        libgcc-s1-dbgsym \
        libstdc++6-dbgsym \
    && rm -rf /var/lib/apt/lists/*
```

Also bump the `cache_bust` line so the image rebuilds.

### Verification

1. Local Dockerfile build: `docker buildx build --platform linux/amd64
   -f .github/workflows/deps/prebuilt.profiling.Dockerfile .` succeeds
   without `apt`/keyring errors.
2. After Release runs the new image in CI:
   `grep -oc '\[unknown\]' flamegraph-cpu-fuzzy-gov-node-1.svg`
   drops to ~1000–1200 (from the current 1712).
3. `[unknown]` reduction must not regress the existing symbols — open the
   SVG and confirm Rust function names still appear at top frames.

### Anti-patterns to avoid

- Do **not** use `libgcc-s1-dbg` / `libstdc++6-dbg` (stale `-dbg`
  names); the current Ubuntu archive ships `-dbgsym` only. The dbgsym
  package's own `Breaks:` rules confirm `-dbg` versions are removed.
- Do **not** use `apt-key add` — deprecated since 22.04, removed in
  24.04. Use the `[signed-by=...]` keyring file syntax.
- Do **not** enable only the `noble` pocket — the runtime `libgcc-s1`
  is at the `noble-updates` (security) version `14.2.0-4ubuntu2~24.04.1`,
  and dbgsym's strict `Depends: libgcc-s1 (= <version>)` won't resolve
  without `noble-updates`. Enable release + updates + proposed (matches
  the wiki).
- Do **not** use `lsb_release -cs` without installing `lsb-release`
  first; minimal `ubuntu:24.04` doesn't ship it. Source `/etc/os-release`
  for `$VERSION_CODENAME` (always present).
- Do **not** use `https://ddebs.ubuntu.com` — the server 301-redirects
  HTTPS → HTTP, adding latency on every `apt-get update`. The wiki uses
  `http://`.

### Alternative (mentioned for completeness, NOT recommended)

`debuginfod` (`https://debuginfod.ubuntu.com`) is the wiki's preferred
path on noble — set `DEBUGINFOD_URLS` and elfutils fetches symbols on
demand. Pros: no image-size hit. Cons: requires outbound HTTPS at
profile-collection time; latency on first miss; `perf script` post-
processing must also have network access. For our CI workflow, baking
`-dbgsym` packages into the image is simpler and avoids a network
dependency on the hot path.

### Rollback

- Single-file Dockerfile change. `git revert <commit>` and re-bump
  `cache_bust`.

### Risk: low

Adds a small (~7 MB) layer of debug symbols. Won't affect any code path;
only `perf script` and friends consume the new debug info. If something
breaks, the symptom is the same broken state as before (no new failure
modes).

---

## Phase 4 — DRY: composite action for harvest+lift (#2363)

### Documentation reference

- Best existing analog in core to copy the skeleton from:
  `.github/actions/download-contracts/action.yml` (lines 1–58). Same
  shape — small composite, multi-step bash body, defaulted inputs.
- Other reference points: `.github/actions/setup-rust-ci/action.yml`
  (inputs with `default`, conditional `if:`),
  `.github/actions/style/action.yml` (multi-step bash chain).

### Files to touch

1. **New file:** `.github/actions/collect-fuzzy-profiling/action.yml`
2. **Modify:** `.github/workflows/fuzzy-load-test.yml`

### What the composite action contains

Wraps the 4 step blocks already in the workflow, taking a single input:

```yaml
name: Collect fuzzy profiling
description: Stop perf, harvest profiling-dump bind mounts, lift reports
  to the dedicated artifact for a fuzzy suite.

inputs:
  suite:
    description: "Fuzzy suite name (drives log/data/reports paths)"
    required: true
  artifact-suffix:
    description: "Artifact-name suffix (defaults to suite; needed because
      `kv-store-with-handlers` uses `handlers` as its artifact suffix)"
    required: false
    default: ""

runs:
  using: composite
  steps:
    # 1. Collect profiling data from containers (collect-from-containers.sh)
    # 2. Graceful merod container shutdown (docker stop --time=120 by name pattern)
    # 3. Collect profiling data from host-side bind mounts (harvest-host-profiling.sh)
    # 4. Collect profiling reports (lift-reports.sh)
    # All steps shell: bash, with ${{ inputs.suite }} substitution.
```

Then in the workflow, each of the 4 suite jobs replaces ~55 lines with:

```yaml
- name: Collect fuzzy profiling
  if: always() && steps.image.outputs.profiling == 'true'
  uses: ./.github/actions/collect-fuzzy-profiling
  with:
    suite: kv-store
```

### Source line ranges of the 4 step-block instances to extract

(per the workflow survey)

- **kv-store**: lines 356–410 (collect 356–364; shutdown 378–394; harvest
  396–402; reports 404–410); uploads 451–468.
- **kv-store-with-handlers**: lines 652–693; uploads 734–749.
- **scaffolding-e2e**: lines 928–967; uploads 1010–1024.
- **group-governance**: lines 1204–1245; uploads 1286–1303.

Implementer should diff these 4 ranges before deduplicating to confirm
they truly only differ in suite name.

### Verification

1. `act` (GitHub Actions local runner) or a workflow-dispatch test run
   exercises the composite action and produces an artifact identical to
   what the inline blocks would have produced.
2. `find .github/workflows/fuzzy-load-test.yml -size -<some smaller value>`
   confirms the file shrank by ~150 lines.
3. Existing CI behavior (one fuzzy run) is byte-identical in artifact
   layout to the pre-refactor run.

### Anti-patterns to avoid

- Don't fold the `if: always() && steps.image.outputs.profiling == 'true'`
  guard into the composite — `steps.image` is the caller's job state and
  is not visible inside the composite. Keep the `if:` on the `uses:`
  line.
- Every `run:` in a composite **must** declare `shell: bash` — unlike
  workflow jobs. Mirror all existing composite actions in core.
- `actions/upload-artifact@v4` is supported inside composites (other
  composites in core use `actions/checkout@v4`, `actions/download-artifact@v4`).

### Rollback

- Two files changed: deleting the action.yml and `git revert` of the
  workflow patch restores byte-identical behavior.

### Risk: medium

Refactor with no functional change — but composite actions have subtle
context differences from inline steps (e.g. `steps.X.outputs.Y` is not
accessible across the composite boundary). A bad refactor could silently
drop one of the steps. Mitigation: diff the 4 line ranges before
extracting; one fuzzy run with the composite to confirm artifact layout
matches.

---

## Phase 5 — (Optional) Real DWARF investigation

Only attempt if frame-pointer flamegraphs prove insufficient for actual
benchmarking work (e.g. analysts complain that the ~1500 `[unknown]`
frames hide important call paths). Otherwise leave as a known-stale
research item.

### Prerequisites (don't start without all four)

1. A local Docker reproducer matching the CI image (pull
   `ghcr.io/calimero-network/merod:edge-profiling` and run interactively).
2. perf source at the version Ubuntu 24.04 ships (`apt source linux-tools-...`
   in a build container).
3. A merod-binary inspector ready (`readelf -wif merod | head -1000`) —
   we suspect post-#2351 merod's DWARF has a new construct that
   `addr2line_configure` chokes on.
4. Permission to instrument the entrypoint or wrap perf record with
   strace temporarily.

### Investigation outline

1. Reproduce the `data size = 0` failure locally on the new merod
   binary.
2. `strace -f -e file -o trace.log perf record -F 99 --call-graph dwarf,16384 -N -p <merod-pid>`
   then SIGINT and grep for `addr2line` invocations in the strace.
3. Read perf source's `tools/perf/util/srcline.c::addr2line_configure`
   to understand the entry condition.
4. Check whether `perf record --no-bpf-event` or similar flags reduce the
   addr2line surface.
5. Consider filing a bug upstream against `linux-perf-tools` if perf
   itself has the bug; otherwise file against `gcc`/`rustc` if the
   binary's DWARF is unparseable.

### Verification (if a fix lands)

`flamegraph-cpu-fuzzy-gov-node-1.svg` `[unknown]` count drops to
< 500 per stack (DWARF unwinding captures libc/jemalloc transitions
correctly).

### Risk: high effort, uncertain payoff

Could be a multi-day deep dive. Frame-pointer flamegraphs are already
usable for benchmarking; treat this as an optimization, not a
necessity.

---

## Final verification (after Phases 1–4 are merged)

Run one workflow_dispatch fuzzy-load-test invocation. Pass criteria, all
on the same run:

1. **All 16 nodes have CPU flamegraphs.** For each of the 4 suites and
   each of nodes 1–4, the suite's `profiling-reports-<suite>-*`
   artifact contains `flamegraph-cpu-fuzzy-<suite>-node-N.svg` ≥ 500 KB
   with a real flame structure (not the encoded-error placeholder).
2. **All 16 nodes have memory flamegraphs** (already works today;
   regression check).
3. **`[unknown]` count.** `grep -oc '\[unknown\]' <each-cpu-svg>` reports
   ≤ 1200 per file.
4. **No silent failures.** Workflow log contains zero
   `perf not compatible, skipping CPU profiling` strings.
5. **No regressions.** PR #2342 + the four follow-ups together do not
   slow the fuzzy job by > 10 % vs the current `daed3a94` baseline
   (composite-action refactor is the only one that could affect runtime,
   and it should be no-op).

If any pass criterion fails, file a specific follow-up issue with the
artifact link and the failing diagnostic line.

---

## Hand-off cheat sheet

| Phase | Files | Verification command | Risk |
|---|---|---|---|
| 1 | (none — wait & merge) | `gh pr view 2342 --json mergeStateStatus` → CLEAN | low |
| 2 | `entrypoint-profiling.sh` (+ optionally Dockerfile) | grep no `perf not compatible` in node logs; per-node harvest report shows `perf.data=1` for all 16 nodes | medium |
| 3 | `prebuilt.profiling.Dockerfile` (one new RUN block + bump cache_bust) | `grep -oc '\[unknown\]'` < 1200 on a sample CPU SVG | low |
| 4 | new `.github/actions/collect-fuzzy-profiling/action.yml`; modify `fuzzy-load-test.yml` | byte-identical artifact layout in a test run | medium |
| 5 | (optional, deep dive) | `[unknown]` < 500 per stack | high effort |

## Background context (subagent reports condensed)

- **Worker perf failure** (subagent 1): all 5 `start_profiling()`
  early-return paths enumerated; in-container logs confirm path B
  (sanity check) is the actual failure. merobox creates seed and worker
  containers *identically* — no architectural seed/worker asymmetry; the
  divergence is the in-container apt race.
- **ddebs setup** (subagent 2): verified against
  `Packages.gz` and `filelist` endpoints; exact RUN block produced above
  is copy-ready.
- **Composite action survey** (subagent 3): 8 composite actions exist in
  core; `download-contracts/action.yml` is the closest skeleton match;
  the only inter-suite-variance is the suite name (+ one artifact-suffix
  edge case for `kv-store-with-handlers`).

---

*Plan authored 2026-05-15. Origin tip at authoring: `daed3a94`.*
*Subagent reports archived in this session's transcript.*
