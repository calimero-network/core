# calimero-sandbox - the job node's Linux sandbox

## Package Identity

- **Crate**: `calimero-sandbox` (`publish = false`); Linux only (elsewhere it builds, and `init_if_requested` does nothing)
- **Entry**: `src/lib.rs` (`Config`, `Image`, `Limits`, `ExecRequest`, `ExecOutcome`, `Usage`, `SandboxError`, the `MAX_*` limits, `init_if_requested`), `run.rs` (`Run`), `init.rs` (the init `exec` re-runs the node's binary as), `cgroup.rs` (v2, with v1 for development hosts), `seccomp.rs`
- **Docs**: [Compute actions](https://calimero-network.github.io/core/protocol/compute-actions/); a sandbox run is a compute run whose component also drives this sandbox

## What This Crate Does

`Run::create(config, id, image, limits)` sets up one sandbox run: a size-capped tmpfs workspace under `Config::base_dir/<id>` and a cgroup `Config::cgroup_name/<id>` capping memory (no swap), processes and CPU (`vcpus` × the CFS period). `Run::exec` starts `Config::init_exe` (default `/proc/self/exe`) with `INIT_ARG`; between fork and exec the child joins the run's cgroup and unshares mount, PID, network, IPC, UTS and cgroup namespaces. Init (`init.rs`) forks PID 1, which builds a fresh root on tmpfs (the image's `usr` read-only, a minimal `/etc` and `/dev`, `/proc`, `/tmp`, `/workspace`), pivots into it, drops every capability, becomes `NOBODY` with `no_new_privs`, installs the seccomp deny-list and runs the command. `Run::write`, `read` and `collect` act on the workspace through `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)` with `O_NONBLOCK` and a regular-file check. Dropping the run kills everything in its cgroup, unmounts the workspace and removes both.

## Contract

- **Init first.** A binary that creates runs calls `init_if_requested()` before it starts a thread or parses arguments (`merod`'s `main` does, before the tokio runtime); the init forks.
- **Workspace I/O is exactly the openat2 resolve flags above**, `O_NONBLOCK` and `require_regular`; a sandbox controls those paths. The escape suite's `host_*` cases guard it.
- **Errors**: `SandboxError::Invalid` is the request's fault (empty argv, a bad env var, a path that leaves the workspace, a symlink or FIFO, over a limit) and is safe to hand to the driver; `Unavailable` is the node's (mounts, cgroups, spawn) and loses the attempt.
- **Limits**: `MAX_EXEC_MS` (900 s per `exec`), `MAX_OUTPUT_BYTES` (1 MiB each of stdout and stderr, then `truncated`), `MAX_READ_BYTES` (16 MiB), `MAX_ARGV`, `MAX_ENV`, `MAX_REQUEST_BYTES`, `MAX_STDIN_BYTES`, checked before anything is spawned.
- **cgroups**: v2 when `/sys/fs/cgroup/cgroup.controllers` offers `memory`, `pids` and `cpu` (a job node; enables them in the root's and `cgroup_name`'s `subtree_control`, kills with `cgroup.kill`); otherwise v1 under each controller's hierarchy, killed in a loop. `Run::usage` reads CPU time and peak memory from whichever is in use.
- **Controls**: the `controls` feature adds `Limits::control` (`NoSeccomp`, `Root`) and lets init accept them; without it init accepts only `full`. Nothing but the escape suite enables it.
- **Not yet**: the init runs as root before dropping privileges (a user namespace with idmapped mounts replaces that), the seccomp filter is a deny-list (an allow-list from the curated images replaces it), the workspace is tmpfs (dm-crypt with a per-run key), and `Image::host()` binds the node's own `/usr` (curated images by digest).

## Tests

```bash
cargo test -p calimero-sandbox                       # request checks; the escape suite skips
sudo env CALIMERO_SANDBOX_PRIVILEGED=1 cargo test -p calimero-sandbox --features controls --test escape
```

`tests/escape.rs` (its own `main`: each `exec` re-runs it as init) runs a demo (Python, a shell pipeline, C, `collect`), cross-run isolation, the 21 attack cases with every layer on (all must be blocked), then with `NoSeccomp` and `Root` (each must go red), and checks no run directory, mount or cgroup is left. CI's `Sandbox escape suite` job runs it as root on cgroup v2.
