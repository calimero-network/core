# Sandbox runs: feasibility prototype

Not part of core's workspace and not shipped. It answers one question: can a
`calimero:app`-style WASM component on wasmtime drive real Linux tools (a shell,
Python, gcc) in an isolated sandbox on a job node, cheaply enough and without
the sandbox reaching the host?

```
driver.wasm (world calimero:sandbox-proto/driver)      wasmtime 51, as in core
   | sandbox.exec / write / read / collect               wit/sandbox.wit
host (sandbox-proto-host)                                host/src/main.rs
   | per run: tmpfs /workspace, cgroups (memory, pids, cpu)    host/src/sandbox.rs
   | per exec: new mount/pid/net/ipc/uts/cgroup namespaces
init (same binary, __sandbox_init)                       host/src/init.rs
   pivot_root into a fresh root: ro /usr, minimal /etc and /dev, /proc, /tmp, /workspace
   drop all capabilities -> nobody, no_new_privs, rlimits, seccomp deny-list
   fork the command, reap as PID 1
```

The host does workspace I/O with `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS |
RESOLVE_NO_XDEV)` and refuses anything but regular files, so a sandbox cannot
plant a symlink or FIFO that makes the host read, write or block outside it.

## Run

```
./run.sh results.json                         # needs root, Linux, rustup target wasm32-wasip2
SANDBOX_PROTO_CONTROL=no-seccomp ./run.sh     # controls: the escape suite must go red
SANDBOX_PROTO_CONTROL=root ./run.sh
```

Tasks (`driver/src/lib.rs`): `demo` (Python, a shell pipeline, compile and run C,
read and collect an output), `escape` (21 attacks that must fail),
`isolation-write`/`isolation-read` (a second run sees nothing of the first),
`bench` (per-exec overhead).

## What it does not cover

TDX (the container is a Firecracker VM: no TD, but also no nested
virtualization, as in a TD), gVisor (not installed here), cgroup v2, a pinned
OCI image (it binds the host's `/usr` read-only), and egress. Results and the
feasibility verdict are in the design doc that accompanies this prototype.
