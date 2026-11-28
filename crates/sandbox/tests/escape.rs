//! The escape suite: each case attacks the sandbox and checks what actually happened, not that
//! a call failed. With `--features controls` it also runs with one layer off (no seccomp; no
//! privilege drop and no seccomp) and fails unless the suite then goes red, so a missing layer
//! cannot pass unnoticed.
//!
//! It needs root on Linux and runs only with `CALIMERO_SANDBOX_PRIVILEGED=1`:
//! `CALIMERO_SANDBOX_PRIVILEGED=1 cargo test -p calimero-sandbox --features controls --test escape`.
//! Each `exec` re-runs this binary as the sandbox's init, so it has its own `main`.
#![allow(clippy::unwrap_used, reason = "a test binary")]

/// The one test this binary holds, as `--list` names it to cargo-nextest.
const TEST_NAME: &str = "escape_suite";

/// Answers libtest's `--list` (one test, none ignored) so cargo-nextest can run this binary.
fn listed() -> bool {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--list") {
        return false;
    }
    if !args.iter().any(|a| a == "--ignored") {
        println!("{TEST_NAME}: test");
    }
    true
}

#[cfg(target_os = "linux")]
fn main() {
    calimero_sandbox::init_if_requested();
    if listed() {
        return;
    }
    if std::env::var("CALIMERO_SANDBOX_PRIVILEGED").as_deref() != Ok("1") {
        println!("escape suite skipped: set CALIMERO_SANDBOX_PRIVILEGED=1 and run as root");
        return;
    }
    suite::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    if listed() {
        return;
    }
    println!("escape suite skipped: sandbox runs need Linux");
}

#[cfg(target_os = "linux")]
mod suite {
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use calimero_sandbox::{Config, ExecOutcome, ExecRequest, Image, Limits, Run};

    const CGROUP: &str = "calimero-sandbox-test";

    fn config() -> Config {
        Config {
            base_dir: PathBuf::from(format!("/run/calimero-sandbox-test-{}", std::process::id())),
            cgroup_name: CGROUP.to_owned(),
            ..Config::default()
        }
    }

    fn create(id: &str, limits: &Limits) -> Run {
        Run::create(&config(), id, Image::host(), limits).unwrap()
    }

    fn exec(run: &Run, argv: &[&str], timeout_ms: u32) -> ExecOutcome {
        run.exec(&ExecRequest {
            argv: argv.iter().map(|s| (*s).to_owned()).collect(),
            timeout_ms,
            ..ExecRequest::default()
        })
        .unwrap()
    }

    fn sh(run: &Run, script: &str) -> ExecOutcome {
        exec(run, &["/bin/sh", "-c", script], 10_000)
    }

    fn py(run: &Run, code: &str, timeout_ms: u32) -> ExecOutcome {
        exec(run, &["python3", "-c", code], timeout_ms)
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).trim().to_owned()
    }

    fn show(r: &ExecOutcome) -> String {
        let cut = |s: String, n: usize| s.chars().take(n).collect::<String>();
        format!(
            "exit {} timed_out {} stdout {:?} stderr {:?}",
            r.exit_code,
            r.timed_out,
            cut(text(&r.stdout), 300),
            cut(text(&r.stderr), 200)
        )
    }

    type Case = (&'static str, fn(&Run) -> (bool, String));

    const CASES: &[Case] = &[
        ("host_files_hidden", |run| {
            let r = sh(run, "for p in /root /home /run/calimero-sandbox-test-* /etc/shadow /proc/1/root/etc/shadow; do test -e $p && echo VISIBLE:$p; done; echo done");
            (
                r.exit_code == 0 && !text(&r.stdout).contains("VISIBLE"),
                show(&r),
            )
        }),
        ("rootfs_read_only", |run| {
            let r = sh(run, "for f in /usr/x /x /etc/passwd /workspace/../x; do (echo h >> $f) 2>/dev/null && echo WROTE:$f || echo refused:$f; done");
            (text(&r.stdout).matches("refused:").count() == 4, show(&r))
        }),
        ("unprivileged", |run| {
            let r = sh(run, "id -u; grep -E '^(CapEff|CapPrm|CapBnd|CapAmb|NoNewPrivs|Seccomp):' /proc/self/status");
            let out = text(&r.stdout);
            let ok = out.starts_with("65534")
                && out.contains("CapEff:\t0000000000000000")
                && out.contains("CapBnd:\t0000000000000000")
                && out.contains("NoNewPrivs:\t1")
                && out.contains("Seccomp:\t2");
            (ok, show(&r))
        }),
        ("node_root_out_of_reach", |run| {
            // Root inside the sandbox must be no uid of the node's that holds power: the user
            // namespace maps no id to the node's 0.
            let r = sh(run, "cat /proc/self/uid_map /proc/self/gid_map");
            let out = text(&r.stdout);
            let maps_node_root = out
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some("0"));
            (
                r.exit_code == 0 && !out.is_empty() && !maps_node_root,
                show(&r),
            )
        }),
        ("proc_masked", |run| {
            // What OCI runtimes mask: the kernel's timers, keys, memory image and hardware
            // listings read empty, and the kernel's switches cannot be written.
            let r = sh(run, "for f in /proc/interrupts /proc/timer_list /proc/keys /proc/kcore /proc/sched_debug /proc/latency_stats; do test -e $f && echo \"$f $(head -c 64 $f 2>/dev/null | wc -c)\"; done; for d in /proc/acpi /proc/scsi /proc/asound; do ls -A $d 2>/dev/null; done | grep -c . ; (echo h > /proc/sysrq-trigger) 2>/dev/null && echo SYSRQ-WRITTEN; (echo 1 > /proc/sys/vm/drop_caches) 2>/dev/null && echo SYS-WRITTEN; echo end");
            let out = text(&r.stdout);
            let leaked = out
                .lines()
                .filter(|l| l.starts_with("/proc/"))
                .any(|l| !l.ends_with(" 0"));
            let listed: u32 = out
                .lines()
                .find(|l| l.chars().all(|c| c.is_ascii_digit()) && !l.is_empty())
                .and_then(|l| l.parse().ok())
                .unwrap_or(999);
            let ok = !leaked && listed == 0 && !out.contains("WRITTEN") && out.ends_with("end");
            (ok, show(&r))
        }),
        ("writes_stay_in_workspace_tmp_and_dev", |run| {
            // Landlock: what the read-only root leaves writable (the process's own `/proc`
            // entries) still is not, outside `/workspace`, `/tmp` and `/dev`.
            let r = sh(run, "for f in /proc/self/comm /proc/self/oom_score_adj /proc/self/clear_refs; do (echo 1 > $f) 2>/dev/null && echo WROTE:$f; done; echo x > /tmp/t && echo x > /workspace/w && echo x > /dev/null && echo end");
            let out = text(&r.stdout);
            (!out.contains("WROTE:") && out.ends_with("end"), show(&r))
        }),
        ("setuid_binaries_inert", |run| {
            let r = sh(
                run,
                "su -c id root </dev/null 2>&1; mount -t tmpfs x /tmp 2>&1; echo end",
            );
            (!text(&r.stdout).contains("uid=0"), show(&r))
        }),
        ("process_isolation", |run| {
            let r = sh(
                run,
                "ls /proc | grep -cE '^[0-9]+$'; kill -9 -1 2>/dev/null; echo still-here",
            );
            let n: u32 = text(&r.stdout)
                .lines()
                .next()
                .and_then(|l| l.parse().ok())
                .unwrap_or(999);
            (n <= 5, show(&r))
        }),
        ("no_network", |run| {
            let r = py(run, "import socket\nfor t in [('1.1.1.1',443),('169.254.169.254',80)]:\n  s=socket.socket();s.settimeout(2)\n  try: s.connect(t);print('CONNECTED',t)\n  except OSError as e: print('blocked',t,e.errno)\ntry: socket.getaddrinfo('example.com',443);print('RESOLVED')\nexcept OSError as e: print('no dns',e.errno)", 10_000);
            let out = text(&r.stdout);
            (
                r.exit_code == 0 && !out.contains("CONNECTED") && !out.contains("RESOLVED"),
                show(&r),
            )
        }),
        ("namespace_and_mount_syscalls", |run| {
            let r = py(run, "import ctypes,os\nl=ctypes.CDLL(None,use_errno=True)\ndef t(n,f):\n  ctypes.set_errno(0);r=f();print(n,r,os.strerror(ctypes.get_errno()))\nt('unshare_user',lambda:l.unshare(0x10000000))\nt('unshare_mnt',lambda:l.unshare(0x20000))\nt('mount',lambda:l.mount(b'none',b'/tmp',b'tmpfs',0,None))\nt('ptrace_attach_1',lambda:l.ptrace(16,1,0,0))\nt('bpf',lambda:l.syscall(321,0,0,0))\nt('io_uring_setup',lambda:l.syscall(425,1,0))\nt('chroot',lambda:l.chroot(b'/tmp'))", 10_000);
            let out = text(&r.stdout);
            (
                out.lines().count() == 7 && out.lines().all(|l| l.contains(" -1 ")),
                show(&r),
            )
        }),
        ("clone_namespace_flags", |run| {
            let r = sh(
                run,
                "unshare --user --map-root-user id 2>&1; unshare -m true 2>&1; echo end",
            );
            (!text(&r.stdout).contains("uid=0"), show(&r))
        }),
        ("kernel_and_proc_knobs", |run| {
            let r = sh(run, "echo 1 > /proc/sys/kernel/sysrq 2>&1 || echo sysrq-denied; dmesg 2>&1 | head -1; head -2 /proc/kallsyms");
            let out = text(&r.stdout);
            let addresses_hidden = !out.lines().any(|l| {
                l.len() > 16
                    && l.chars().take(16).all(|c| c.is_ascii_hexdigit())
                    && !l.starts_with("0000000000000000")
            });
            (out.contains("sysrq-denied") && addresses_hidden, show(&r))
        }),
        ("cgroup_view", |run| {
            let r = sh(run, "cat /proc/self/cgroup | head -3; test -d /sys/fs/cgroup && echo CGROUPFS-VISIBLE; echo end");
            let out = text(&r.stdout);
            (
                !out.contains("CGROUPFS-VISIBLE") && !out.contains(CGROUP),
                show(&r),
            )
        }),
        ("env_clean", |run| {
            let r = sh(run, "env | cut -d= -f1 | sort | tr '\\n' ' '");
            let allowed = ["HOME", "LANG", "PATH", "PWD", "SHLVL", "_", "OLDPWD"];
            (
                text(&r.stdout)
                    .split_whitespace()
                    .all(|k| allowed.contains(&k)),
                show(&r),
            )
        }),
        ("fork_bomb_contained", |run| {
            let r = py(run, "import os,time\nn=0\ntry:\n  while True:\n    if os.fork()==0:\n      time.sleep(60);os._exit(0)\n    n+=1\nexcept OSError as e: print('forks',n,'errno',e.errno)", 10_000);
            let n: u32 = text(&r.stdout)
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(999);
            (n < 64 && !r.timed_out, show(&r))
        }),
        ("memory_capped", |run| {
            let r = py(
                run,
                "b=bytearray(1<<30)\nfor i in range(0,len(b),4096): b[i]=1\nprint('ALLOCATED')",
                20_000,
            );
            (
                !text(&r.stdout).contains("ALLOCATED") && r.exit_code == 137,
                show(&r),
            )
        }),
        ("cpu_timeout", |run| {
            let r = exec(run, &["/bin/sh", "-c", "while :; do :; done"], 1_500);
            (r.timed_out && r.wall_us < 3_000_000, show(&r))
        }),
        ("orphans_reaped", |run| {
            let r = sh(run, "sleep 300 & echo started");
            let r2 = sh(run, "ps -e -o comm= 2>/dev/null | grep -c sleep || true");
            (
                text(&r2.stdout) == "0" && r.wall_us < 3_000_000,
                format!("{} / {}", show(&r), show(&r2)),
            )
        }),
        ("workspace_quota", |run| {
            let r = sh(run, "dd if=/dev/zero of=/workspace/big bs=1M count=200 2>&1 | tail -1; rm -f /workspace/big");
            let out = text(&r.stdout);
            (
                out.contains("No space left") || out.contains("67108864"),
                show(&r),
            )
        }),
        ("output_capped", |run| {
            let r = sh(run, "head -c 5242880 /dev/zero | tr '\\0' a");
            (
                r.truncated && r.stdout.len() == 1 << 20,
                format!("truncated {} bytes {}", r.truncated, r.stdout.len()),
            )
        }),
        ("host_read_symlink_attack", |run| {
            let _ = sh(run, "ln -sf /etc/shadow leak; ln -sf ../../../../../../etc/shadow leak2; ln -sf /proc/1/root/etc/shadow leak3; mkdir -p d; ln -sfn / d/root");
            let results: Vec<String> = ["leak", "leak2", "leak3", "d/root/etc/shadow"]
                .iter()
                .map(|p| format!("{p}: {:?}", run.read(p, 64).map(|b| text(&b))))
                .collect();
            (
                results.iter().all(|r| r.contains("Err")),
                results.join("; "),
            )
        }),
        ("host_write_symlink_attack", |run| {
            let _ = sh(run, "ln -sf /tmp/pwned-by-sandbox target");
            let r = run.write("target", b"x");
            (
                r.is_err() && !Path::new("/tmp/pwned-by-sandbox").exists(),
                format!("{r:?}"),
            )
        }),
        ("host_fifo_trap", |run| {
            let _ = sh(run, "mkfifo pipe");
            let read = run.read("pipe", 64);
            let collect = run.collect("pipe").map(|(_, n)| n);
            (
                read.is_err() && collect.is_err(),
                format!("{read:?} {collect:?}"),
            )
        }),
        ("host_path_traversal", |run| {
            let rs: Vec<String> = ["../escape", "/etc/x", "a/../../x", "/proc/self/root/x"]
                .iter()
                .map(|p| format!("{p}: {:?}", run.write(p, b"x")))
                .collect();
            (rs.iter().all(|r| r.contains("Err")), rs.join("; "))
        }),
    ];

    /// Runs every case, each in a fresh run, and returns how many held.
    fn escape_suite(limits: &Limits, label: &str) -> usize {
        let mut passed = 0;
        for (i, (name, case)) in CASES.iter().enumerate() {
            let run = create(&format!("{label}-{i}"), limits);
            let (ok, evidence) = case(&run);
            println!("  {} {name}: {evidence}", if ok { "ok  " } else { "FAIL" });
            passed += usize::from(ok);
        }
        println!("{label}: {passed}/{} blocked", CASES.len());
        passed
    }

    /// Python, a shell pipeline, C, and a file taken out of the run.
    fn demo() {
        let run = create("demo", &Limits::default());
        run.write("data.csv", b"alice,3\nbob,5\nalice,4\ncarol,1\nbob,2\n")
            .unwrap();
        run.write(
            "report.py",
            b"import csv, collections, json\nt = collections.Counter()\nfor n, v in csv.reader(open('data.csv')): t[n] += int(v)\njson.dump(t, open('report.json', 'w'), sort_keys=True)\nprint('ok', sum(t.values()))\n",
        )
        .unwrap();
        let r = exec(&run, &["python3", "report.py"], 10_000);
        assert_eq!(
            (r.exit_code, text(&r.stdout).as_str()),
            (0, "ok 15"),
            "{}",
            show(&r)
        );
        let r = sh(
            &run,
            "cut -d, -f1 data.csv | sort | uniq -c | sort -rn | head -1",
        );
        assert!(
            text(&r.stdout).ends_with("alice") || text(&r.stdout).ends_with("bob"),
            "{}",
            show(&r)
        );
        run.write(
            "hello.c",
            b"#include <stdio.h>\nint main(){puts(\"hello from C\");return 0;}\n",
        )
        .unwrap();
        let r = sh(&run, "gcc -O2 -o hello hello.c && ./hello");
        assert_eq!(text(&r.stdout), "hello from C", "{}", show(&r));
        let (mut f, len) = run.collect("report.json").unwrap();
        let mut body = String::new();
        let _ = f.read_to_string(&mut body).unwrap();
        assert_eq!(body, r#"{"alice": 7, "bob": 7, "carol": 1}"#);
        assert_eq!(len, body.len() as u64);
        let usage = run.usage();
        assert!(usage.cpu_us > 0 && usage.peak_memory_bytes > 0, "{usage:?}");
        println!("demo: ok, {usage:?}");
    }

    /// A second run sees nothing the first left in its workspace or `/tmp`.
    fn isolation() {
        let first = create("iso-a", &Limits::default());
        first.write("secret.txt", b"run A's data").unwrap();
        let _ = sh(&first, "echo a > /workspace/a.txt; echo t > /tmp/t.txt");
        drop(first);
        let second = create("iso-b", &Limits::default());
        let r = sh(&second, "ls -A /workspace /tmp | grep -v ':$' | grep -c . ; cat /workspace/secret.txt 2>/dev/null; true");
        assert_eq!(text(&r.stdout), "0", "{}", show(&r));
        println!("isolation: ok");
    }

    /// Nothing of any run is left on the node: no run directory, mount or cgroup.
    fn nothing_left() {
        let base = config().base_dir;
        let dirs = fs::read_dir(&base).map(|d| d.count()).unwrap_or(0);
        let mounts = fs::read_to_string("/proc/self/mountinfo").unwrap();
        let mounted = mounts
            .lines()
            .filter(|l| l.contains(base.to_str().unwrap()))
            .count();
        let cgroups: usize = ["", "memory", "pids", "cpu", "cpuacct"]
            .iter()
            .map(|c| Path::new("/sys/fs/cgroup").join(c).join(CGROUP))
            .filter_map(|d| fs::read_dir(d).ok())
            .map(|d| {
                d.filter(|e| e.as_ref().is_ok_and(|e| e.path().is_dir()))
                    .count()
            })
            .sum();
        assert_eq!(
            (dirs, mounted, cgroups),
            (0, 0, 0),
            "run directories, mounts, cgroups left"
        );
        let _ = fs::remove_dir(&base);
        println!("nothing left: ok");
    }

    pub(super) fn run() {
        demo();
        isolation();
        let full = escape_suite(&Limits::default(), "full");
        controls();
        nothing_left();
        assert_eq!(full, CASES.len(), "the escape suite must block every case");
    }

    #[cfg(feature = "controls")]
    fn controls() {
        use calimero_sandbox::Control;
        for control in [
            Control::NoSeccomp,
            Control::Root,
            Control::NoUserns,
            Control::NoLandlock,
        ] {
            let limits = Limits {
                control,
                ..Limits::default()
            };
            let passed = escape_suite(&limits, &format!("{control:?}").to_lowercase());
            assert!(
                passed < CASES.len(),
                "{control:?}: the suite must go red without that layer"
            );
        }
    }

    #[cfg(not(feature = "controls"))]
    fn controls() {
        println!("controls skipped: run with --features controls");
    }
}
