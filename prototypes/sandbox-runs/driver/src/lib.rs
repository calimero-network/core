//! Prototype driver component. Runs on the job node as WASM and steps through commands
//! in the run's sandbox through the host's `sandbox` interface; returns JSON.
//!
//! Tasks: `demo` (a small build-and-collect workflow), `escape` (attacks that must fail),
//! `isolation-write` / `isolation-read` (a second run must not see the first's files),
//! `bench` (per-exec overhead).

wit_bindgen::generate!({ path: "../wit", world: "driver" });

use calimero::sandbox_proto::sandbox::{self as sb, ExecRequest, ExecResult};
use serde_json::{json, Value};

struct Driver;
export!(Driver);

impl Guest for Driver {
    fn run(task: String, _args: Vec<u8>) -> Result<Vec<u8>, String> {
        let v = match task.as_str() {
            "demo" => demo()?,
            "escape" => escape(),
            "isolation-write" => isolation_write()?,
            "isolation-read" => isolation_read()?,
            "bench" => bench()?,
            other => return Err(format!("unknown task {other}")),
        };
        serde_json::to_vec(&v).map_err(|e| e.to_string())
    }
}

fn exec_with(argv: &[&str], stdin: &[u8], timeout_ms: u32) -> Result<ExecResult, String> {
    sb::exec(&ExecRequest {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: vec![],
        cwd: None,
        stdin: stdin.to_vec(),
        timeout_ms,
    })
}

fn sh(script: &str) -> Result<ExecResult, String> {
    exec_with(&["/bin/sh", "-c", script], b"", 10_000)
}

fn py(code: &str, timeout_ms: u32) -> Result<ExecResult, String> {
    exec_with(&["python3", "-c", code], b"", timeout_ms)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).trim().to_string()
}

fn summary(r: &ExecResult) -> Value {
    json!({
        "exit": r.exit_code, "timed_out": r.timed_out, "truncated": r.truncated,
        "stdout": text(&r.stdout).chars().take(400).collect::<String>(),
        "stderr": text(&r.stderr).chars().take(300).collect::<String>(),
        "wall_ms": r.wall_us as f64 / 1000.0,
    })
}

// ---- demo -----------------------------------------------------------------------------

fn demo() -> Result<Value, String> {
    let mut steps = vec![];
    sb::write("data.csv", b"alice,3\nbob,5\nalice,4\ncarol,1\nbob,2\n")?;
    sb::write(
        "report.py",
        br#"
import csv, collections, json, platform, sys
totals = collections.Counter()
for name, n in csv.reader(open("data.csv")):
    totals[name] += int(n)
json.dump({"totals": totals, "python": platform.python_version()}, open("report.json", "w"))
print("ok", sum(totals.values()))
"#,
    )?;
    let r = exec_with(&["python3", "report.py"], b"", 10_000)?;
    steps.push(json!({"step": "python3 report.py", "r": summary(&r)}));
    let r = sh("cut -d, -f1 data.csv | sort | uniq -c | sort -rn > counts.txt && cat counts.txt")?;
    steps.push(json!({"step": "shell pipeline", "r": summary(&r)}));
    let r = exec_with(&["gcc", "--version"], b"", 10_000)?;
    steps.push(json!({"step": "gcc --version", "r": summary(&r)}));
    sb::write(
        "hello.c",
        b"#include <stdio.h>\nint main(){puts(\"hello from C in the sandbox\");return 0;}\n",
    )?;
    let r = sh("gcc -O2 -o hello hello.c && ./hello")?;
    steps.push(json!({"step": "compile + run C", "r": summary(&r)}));
    let report = sb::read("report.json", 4096)?;
    let id = sb::collect("report.json")?;
    Ok(json!({
        "steps": steps,
        "report_json": text(&report),
        "collected_output_sha256": id,
    }))
}

// ---- escape suite ---------------------------------------------------------------------

struct Case {
    name: &'static str,
    attack: &'static str,
    pass: bool,
    evidence: Value,
}

fn case(
    name: &'static str,
    attack: &'static str,
    f: impl FnOnce() -> Result<(bool, Value), String>,
) -> Case {
    match f() {
        Ok((pass, evidence)) => Case {
            name,
            attack,
            pass,
            evidence,
        },
        Err(e) => Case {
            name,
            attack,
            pass: false,
            evidence: json!({"driver_error": e}),
        },
    }
}

fn escape() -> Value {
    let cases = vec![
        case(
            "host_files_hidden",
            "look for the host's files (repo, /root, /home)",
            || {
                let r = sh("for p in /home/user/core/CLAUDE.md /root /home /run/sandbox-proto /etc/shadow; do test -e $p && echo VISIBLE:$p; done; echo done")?;
                Ok((
                    r.exit_code == 0 && !text(&r.stdout).contains("VISIBLE"),
                    summary(&r),
                ))
            },
        ),
        case("rootfs_read_only", "write to /usr, /, /etc", || {
            let r = sh("for f in /usr/x /x /etc/passwd /workspace/../x; do (echo h >> $f) 2>/dev/null && echo WROTE:$f || echo refused:$f; done")?;
            let out = text(&r.stdout);
            Ok((
                out.matches("refused:").count() == 4
                    && !out.contains("WROTE:/usr")
                    && !out.contains("WROTE:/etc")
                    && !out.contains("WROTE:/x"),
                summary(&r),
            ))
        }),
        case("unprivileged", "check uid and capabilities", || {
            let r = sh("id -u; grep -E '^(CapEff|CapPrm|CapBnd|CapAmb|NoNewPrivs|Seccomp):' /proc/self/status")?;
            let out = text(&r.stdout);
            let ok = out.starts_with("65534")
                && out.contains("CapEff:\t0000000000000000")
                && out.contains("CapBnd:\t0000000000000000")
                && out.contains("NoNewPrivs:\t1")
                && out.contains("Seccomp:\t2");
            Ok((ok, summary(&r)))
        }),
        case(
            "setuid_binaries_inert",
            "run a setuid-root binary (su, mount)",
            || {
                let r = sh("ls -l /usr/bin/su /usr/bin/mount 2>&1 | cut -c1-10; su -c id root </dev/null 2>&1; mount -t tmpfs x /tmp 2>&1; echo end")?;
                let out = text(&r.stdout);
                Ok((
                    !out.contains("uid=0") && !out.contains("\nroot"),
                    summary(&r),
                ))
            },
        ),
        case(
            "process_isolation",
            "list processes; signal everything visible",
            || {
                let r =
                    sh("ls /proc | grep -cE '^[0-9]+$'; kill -9 -1 2>/dev/null; echo still-here")?;
                let n: u32 = text(&r.stdout)
                    .lines()
                    .next()
                    .and_then(|l| l.parse().ok())
                    .unwrap_or(999);
                Ok((n <= 5, summary(&r)))
            },
        ),
        case("no_network", "connect out by IP and by name", || {
            let r = py(
                "import socket\nfor t in [('1.1.1.1',443),('169.254.169.254',80)]:\n  s=socket.socket();s.settimeout(2)\n  try: s.connect(t);print('CONNECTED',t)\n  except OSError as e: print('blocked',t,e.errno)\ntry: socket.getaddrinfo('example.com',443);print('RESOLVED')\nexcept OSError as e: print('no dns',e.errno)\nimport os;print(sorted(os.listdir('/sys/class/net')) if os.path.exists('/sys/class/net') else 'no /sys')",
                10_000,
            )?;
            let out = text(&r.stdout);
            Ok((
                !out.contains("CONNECTED") && !out.contains("RESOLVED"),
                summary(&r),
            ))
        }),
        case(
            "namespace_and_mount_syscalls",
            "raw unshare(), setns, mount(), ptrace(), bpf via ctypes",
            || {
                let r = py(
                "import ctypes,os\nl=ctypes.CDLL(None,use_errno=True)\ndef t(n,f):\n  ctypes.set_errno(0);r=f();print(n,r,os.strerror(ctypes.get_errno()))\nt('unshare_user',lambda:l.unshare(0x10000000))\nt('unshare_mnt',lambda:l.unshare(0x20000))\nt('mount',lambda:l.mount(b'none',b'/tmp',b'tmpfs',0,None))\nt('ptrace_attach_1',lambda:l.ptrace(16,1,0,0))\nt('bpf',lambda:l.syscall(321,0,0,0))\nt('io_uring_setup',lambda:l.syscall(425,1,0))\nt('chroot',lambda:l.chroot(b'/tmp'))",
                10_000,
            )?;
                let out = text(&r.stdout);
                let ok = out.lines().count() == 7 && out.lines().all(|l| l.contains(" -1 "));
                Ok((ok, summary(&r)))
            },
        ),
        case(
            "clone_namespace_flags",
            "fork into a new user namespace through clone()",
            || {
                let r =
                    sh("unshare --user --map-root-user id 2>&1; unshare -m true 2>&1; echo end")?;
                let out = text(&r.stdout);
                Ok((!out.contains("uid=0"), summary(&r)))
            },
        ),
        case(
            "kernel_and_proc_knobs",
            "write /proc/sys, read dmesg and kallsyms",
            || {
                let r = sh("echo 1 > /proc/sys/kernel/sysrq 2>&1 || echo sysrq-denied; dmesg 2>&1 | head -1; head -2 /proc/kallsyms; ls /sys 2>&1 | head -1")?;
                let out = text(&r.stdout);
                let kallsyms_zeroed = out
                    .lines()
                    .filter(|l| l.starts_with("0000000000000000"))
                    .count()
                    >= 1
                    || !out
                        .lines()
                        .any(|l| l.len() > 16 && l.chars().take(16).all(|c| c.is_ascii_hexdigit()));
                Ok((out.contains("sysrq-denied") && kallsyms_zeroed, summary(&r)))
            },
        ),
        case("cgroup_view", "find and escape the run's cgroup", || {
            let r = sh("cat /proc/self/cgroup | head -3; test -d /sys/fs/cgroup && echo CGROUPFS-VISIBLE; echo end")?;
            let out = text(&r.stdout);
            Ok((
                !out.contains("CGROUPFS-VISIBLE") && !out.contains("sandbox-proto"),
                summary(&r),
            ))
        }),
        case(
            "env_clean",
            "read the environment for host secrets or proxies",
            || {
                let r = sh("env | cut -d= -f1 | sort | tr '\\n' ' '")?;
                let keys: Vec<String> = text(&r.stdout)
                    .split_whitespace()
                    .map(String::from)
                    .collect();
                let allowed = ["HOME", "LANG", "PATH", "PWD", "SHLVL", "_", "OLDPWD"];
                Ok((
                    keys.iter().all(|k| allowed.contains(&k.as_str())),
                    summary(&r),
                ))
            },
        ),
        case(
            "fork_bomb_contained",
            "fork until it fails (pids.max = 64)",
            || {
                let r = py(
                "import os,time\nn=0\ntry:\n  while True:\n    if os.fork()==0:\n      time.sleep(60);os._exit(0)\n    n+=1\nexcept OSError as e: print('forks',n,'errno',e.errno)",
                10_000,
            )?;
                let out = text(&r.stdout);
                let n: u32 = out
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(999);
                Ok((n < 64 && !r.timed_out, summary(&r)))
            },
        ),
        case("memory_capped", "allocate 1 GiB (limit 256 MiB)", || {
            let r = py(
                "b=bytearray(1<<30)\nfor i in range(0,len(b),4096): b[i]=1\nprint('ALLOCATED')",
                20_000,
            )?;
            Ok((
                !text(&r.stdout).contains("ALLOCATED") && r.exit_code == 137,
                summary(&r),
            ))
        }),
        case("cpu_timeout", "spin forever (timeout 1.5 s)", || {
            let r = exec_with(&["/bin/sh", "-c", "while :; do :; done"], b"", 1_500)?;
            Ok((r.timed_out && r.wall_us < 3_000_000, summary(&r)))
        }),
        case(
            "orphans_reaped",
            "leave a background process behind",
            || {
                let r = sh("sleep 300 & echo started")?;
                // The next exec must not see it: a new PID namespace, and the cgroup was emptied.
                let r2 = sh("ps -e -o comm= 2>/dev/null | grep -c sleep || true")?;
                Ok((
                    text(&r2.stdout) == "0" && r.wall_us < 3_000_000,
                    json!({"first": summary(&r), "second": summary(&r2)}),
                ))
            },
        ),
        case(
            "workspace_quota",
            "fill the workspace (64 MiB tmpfs)",
            || {
                let r = sh("dd if=/dev/zero of=/workspace/big bs=1M count=200 2>&1 | tail -1; rm -f /workspace/big")?;
                Ok((
                    text(&r.stdout).contains("No space left")
                        || text(&r.stdout).contains("67108864"),
                    summary(&r),
                ))
            },
        ),
        case("output_capped", "print 5 MiB (cap 1 MiB)", || {
            let r = sh("head -c 5242880 /dev/zero | tr '\\0' a")?;
            Ok((
                r.truncated && r.stdout.len() == 1 << 20,
                json!({"truncated": r.truncated, "stdout_bytes": r.stdout.len()}),
            ))
        }),
        case(
            "host_read_symlink_attack",
            "plant symlinks to host files, ask the host to read them",
            || {
                sh("ln -sf /etc/shadow leak; ln -sf ../../../../../../etc/shadow leak2; ln -sf /proc/1/root/etc/shadow leak3; mkdir -p d; ln -sfn / d/root")?;
                let results: Vec<Value> = ["leak", "leak2", "leak3", "d/root/etc/shadow"]
                    .iter()
                    .map(|p| match sb::read(p, 64) {
                        Ok(b) => json!({"path": p, "Ok": text(&b)}),
                        Err(e) => json!({"path": p, "Err": e}),
                    })
                    .collect();
                let ok = results.iter().all(|v| v.get("Err").is_some());
                Ok((ok, json!(results)))
            },
        ),
        case(
            "host_write_symlink_attack",
            "plant a symlink, ask the host to write through it",
            || {
                sh("ln -sf /tmp/pwned-by-sandbox target")?;
                let r = sb::write("target", b"x");
                Ok((r.is_err(), json!({"write": format!("{r:?}")})))
            },
        ),
        case(
            "host_fifo_trap",
            "plant a FIFO so the host blocks reading it",
            || {
                sh("mkfifo pipe")?;
                let r = sb::read("pipe", 64);
                Ok((r.is_err(), json!({"read": format!("{r:?}")})))
            },
        ),
        case(
            "host_path_traversal",
            "write outside the workspace by path",
            || {
                let rs: Vec<String> = ["../escape", "/etc/x", "a/../../x", "/proc/self/root/x"]
                    .iter()
                    .map(|p| format!("{p}: {:?}", sb::write(p, b"x")))
                    .collect();
                Ok((rs.iter().all(|r| r.contains("Err")), json!(rs)))
            },
        ),
    ];
    let passed = cases.iter().filter(|c| c.pass).count();
    json!({
        "passed": passed,
        "total": cases.len(),
        "cases": cases.iter().map(|c| json!({
            "name": c.name, "attack": c.attack, "pass": c.pass, "evidence": c.evidence,
        })).collect::<Vec<_>>(),
    })
}

// ---- isolation across runs ------------------------------------------------------------

fn isolation_write() -> Result<Value, String> {
    sb::write("secret.txt", b"run A's data")?;
    let r = sh("echo also-from-shell > /workspace/a.txt; echo tmp > /tmp/t.txt; ls /workspace")?;
    Ok(json!({"wrote": summary(&r)}))
}

fn isolation_read() -> Result<Value, String> {
    // Run A left secret.txt and a.txt in /workspace and t.txt in /tmp.
    let r = sh("echo entries=$(ls -A /workspace /tmp | grep -v ':$' | grep -c .); cat /workspace/secret.txt 2>/dev/null; echo end")?;
    let out = text(&r.stdout);
    let pass = out.starts_with("entries=0") && !out.contains("run A");
    Ok(json!({"pass": pass, "listing": summary(&r)}))
}

// ---- bench ----------------------------------------------------------------------------

fn median(mut v: Vec<u64>) -> f64 {
    v.sort();
    v[v.len() / 2] as f64 / 1000.0
}

fn bench() -> Result<Value, String> {
    let mut t = vec![];
    for _ in 0..21 {
        t.push(exec_with(&["true"], b"", 5_000)?.wall_us);
    }
    let mut p = vec![];
    for _ in 0..11 {
        p.push(py("pass", 5_000)?.wall_us);
    }
    // 1 MiB (the output cap) through stdin and back; 4 MiB through host I/O.
    let mib = vec![b'x'; 1 << 20];
    let r = exec_with(&["cat"], &mib, 10_000)?;
    let big = vec![b'x'; 4 << 20];
    let started = std::time::Instant::now();
    sb::write("blob.bin", &big)?;
    let back = sb::read("blob.bin", 8 << 20)?;
    let io_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(json!({
        "exec_true_median_ms": median(t),
        "exec_python_median_ms": median(p),
        "cat_1mib_ms": r.wall_us as f64 / 1000.0,
        "cat_1mib_ok": r.stdout.len() == mib.len() && !r.truncated,
        "host_write_read_4mib_ms": io_ms,
        "host_io_ok": back.len() == big.len(),
    }))
}
