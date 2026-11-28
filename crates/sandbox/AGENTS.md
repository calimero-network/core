# calimero-sandbox - the job node's Linux sandbox

## Package Identity

- **Crate**: `calimero-sandbox` (`publish = false`); Linux only (elsewhere it builds, and `init_if_requested` does nothing)
- **Entry**: `src/lib.rs` (`Config`, `Image`, `Limits`, `ExecRequest`, `ExecOutcome`, `Usage`, `SandboxError`, the `MAX_*` limits, `IMAGES`, `init_if_requested`), `run.rs` (`Run`), `workspace.rs` (`Workspace`: the node's reads and writes of a workspace), `init.rs` (the init `exec` re-runs the node's binary as), `cgroup.rs` (v2, with v1 for development hosts), `seccomp.rs` (the allow-list), `landlock.rs`
- **Docs**: [Compute actions](https://calimero-network.github.io/core/protocol/compute-actions/); a sandbox run is a compute run whose component also drives this sandbox

## What This Crate Does

`Run::create(config, id, image, limits)` sets up one sandbox run: a size-capped tmpfs workspace under `Config::base_dir/<id>` and a cgroup `Config::cgroup_name/<id>` capping memory (no swap), processes and CPU (`vcpus` × the CFS period). `Run::exec` starts `Config::init_exe` (default `/proc/self/exe`) with `INIT_ARG`; between fork and exec the child joins the run's cgroup. Init (`init.rs`) forks a child that unshares a user namespace with mount, PID, network, IPC, UTS and cgroup namespaces; init maps the child's ids `0..65536` to the node's `Config::uid_base..+65536` (`DEFAULT_UID_BASE`, 10^9), so nothing in the sandbox is ever the node's root. The child becomes root of that namespace only and forks PID 1, which builds a fresh root on tmpfs (the image's `usr` read-only, a minimal `/etc` and `/dev`, `/proc` with OCI's masked and read-only entries, `/tmp`, `/workspace`), pivots into it, drops every capability, becomes `NOBODY` with `no_new_privs`, applies a Landlock ruleset (writes only beneath `/workspace`, `/tmp` and `/dev`), installs the seccomp allow-list and runs the command. The workspace is owned by the node's `uid_base + NOBODY`. `Run::write`, `read` and `collect` act on the workspace through `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)` with `O_NONBLOCK` and a regular-file check. Dropping the run kills everything in its cgroup, unmounts the workspace and removes both.

## Contract

- **Init first.** A binary that creates runs calls `init_if_requested()` before it starts a thread or parses arguments (`merod`'s `main` does, before the tokio runtime); the init forks.
- **Workspace I/O is exactly the openat2 resolve flags above**, `O_NONBLOCK` and `require_regular`; a sandbox controls those paths. The escape suite's `host_*` cases guard it.
- **Errors**: `SandboxError::Invalid` is the request's fault (empty argv, a bad env var, a path that leaves the workspace, a symlink or FIFO, over a limit) and is safe to hand to the driver; `Unavailable` is the node's (mounts, cgroups, spawn) and loses the attempt.
- **Limits**: `MAX_EXEC_MS` (900 s per `exec`), `MAX_OUTPUT_BYTES` (1 MiB each of stdout and stderr, then `truncated`), `MAX_READ_BYTES` (16 MiB), `MAX_ARGV`, `MAX_ENV`, `MAX_REQUEST_BYTES`, `MAX_STDIN_BYTES`, checked before anything is spawned.
- **cgroups**: v2 when `/sys/fs/cgroup/cgroup.controllers` offers `memory`, `pids` and `cpu` (a job node; enables them in the root's and `cgroup_name`'s `subtree_control`, kills with `cgroup.kill`); otherwise v1 under each controller's hierarchy, killed in a loop. `Run::usage` reads CPU time and peak memory from whichever is in use.
- **Seccomp** (`seccomp.rs`): an allow-list; anything else answers `ENOSYS` (so libc falls back), `clone` with a namespace flag `EPERM`, and `clone3` is left off so libc uses `clone`. `nothing_that_reaches_outside_the_process_is_allowed` pins what must stay off it.
- **Landlock** (`landlock.rs`, raw syscalls): read and execute anywhere, write, create and remove only beneath `/workspace`, `/tmp` and `/dev`, with the rights the kernel's ABI knows. A kernel without Landlock fails the run (the attempt is lost); it is not skipped.
- **Controls**: the `controls` feature adds `Limits::control` (`NoSeccomp`, `Root` (no privilege drop, seccomp or Landlock), `NoUserns`, `NoLandlock`) and lets init accept them; without it init accepts only `full`. Each removes one layer and the suite shows which cases it alone holds. Nothing but the escape suite enables it.
- **Not yet**: the workspace is tmpfs (dm-crypt with a per-run key; the development container's kernel has no dm-crypt), and `Image::host()` binds the node's own `/usr` (curated images by digest).

## Tests

```bash
cargo test -p calimero-sandbox                       # request checks; the escape suite skips
sudo env CALIMERO_SANDBOX_PRIVILEGED=1 cargo test -p calimero-sandbox --features controls --test escape
```

`tests/escape.rs` (its own `main`: each `exec` re-runs it as init) runs a demo (Python, a shell pipeline, C, `collect`), cross-run isolation, the 24 attack cases with every layer on (all must be blocked), then under each control (each must go red), and checks no run directory, mount or cgroup is left. CI's `Sandbox escape suite` job runs it as root on cgroup v2. `tests/workspace.rs` attacks `Workspace` without root (every CI run), and `fuzz/fuzz_targets/sandbox_workspace.rs` fuzzes it.
