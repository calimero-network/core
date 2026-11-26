"""Sandbox runs: feasibility report, built from the prototype's recorded results."""
import json
import statistics
import sys

from reportlab.graphics.shapes import Drawing, Line, Polygon, Rect, String
from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
from reportlab.lib.units import mm
from reportlab.platypus import (KeepTogether, Paragraph, SimpleDocTemplate, Spacer, Table,
                                TableStyle)

RESULTS, OUT = sys.argv[1], sys.argv[2]
R = json.load(open(RESULTS))

ss = getSampleStyleSheet()
ACCENT = colors.HexColor("#1f4e79")
GRID = colors.HexColor("#c8d1db")
HEAD_BG = colors.HexColor("#e8eef5")
OK = colors.HexColor("#1e7b34")
WARN = colors.HexColor("#9a5b00")
MUTED = colors.HexColor("#555555")

body = ParagraphStyle("b", parent=ss["Normal"], fontName="Helvetica", fontSize=9.5, leading=13.5, spaceAfter=5)
small = ParagraphStyle("s", parent=body, fontSize=8.3, leading=11, spaceAfter=0)
smallb = ParagraphStyle("sb", parent=small, fontName="Helvetica-Bold")
h1 = ParagraphStyle("h1", parent=ss["Title"], fontName="Helvetica-Bold", fontSize=20, leading=24, alignment=0, textColor=ACCENT, spaceAfter=4)
sub = ParagraphStyle("sub", parent=body, fontSize=9, textColor=MUTED, spaceAfter=10)
h2 = ParagraphStyle("h2", parent=ss["Heading2"], fontName="Helvetica-Bold", fontSize=13, textColor=ACCENT, spaceBefore=10, spaceAfter=5)
h3 = ParagraphStyle("h3", parent=ss["Heading3"], fontName="Helvetica-Bold", fontSize=10.5, spaceBefore=6, spaceAfter=3)
bullet = ParagraphStyle("bl", parent=body, leftIndent=12, bulletIndent=2, spaceAfter=2.5)
verdict = ParagraphStyle("v", parent=body, fontSize=10, leading=14.5, spaceAfter=0)


def P(t, s=body):
    return Paragraph(t, s)


def B(items):
    return [Paragraph(i, bullet, bulletText="•") for i in items]


def C(t):
    return f'<font face="Courier" size="8.3">{t}</font>'


def table(rows, widths, zebra=False):
    data = [[c if not isinstance(c, str) else Paragraph(c, smallb if r == 0 else small) for c in row]
            for r, row in enumerate(rows)]
    t = Table(data, colWidths=widths, repeatRows=1)
    style = [
        ("BACKGROUND", (0, 0), (-1, 0), HEAD_BG),
        ("GRID", (0, 0), (-1, -1), 0.5, GRID),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("TOPPADDING", (0, 0), (-1, -1), 3.5), ("BOTTOMPADDING", (0, 0), (-1, -1), 3.5),
        ("LEFTPADDING", (0, 0), (-1, -1), 5), ("RIGHTPADDING", (0, 0), (-1, -1), 5),
    ]
    if zebra:
        for i in range(2, len(rows), 2):
            style.append(("BACKGROUND", (0, i), (-1, i), colors.HexColor("#f7f9fb")))
    t.setStyle(TableStyle(style))
    return t


def footer(canvas, doc):
    canvas.saveState()
    canvas.setFont("Helvetica", 7.5)
    canvas.setFillColor(colors.HexColor("#777777"))
    canvas.drawString(18 * mm, 10 * mm, "Calimero Core — Sandbox runs: feasibility report")
    canvas.drawRightString(A4[0] - 18 * mm, 10 * mm, f"Page {doc.page}")
    canvas.restoreState()


W = A4[0] - 36 * mm

# ---- numbers from the recorded run ------------------------------------------------------
esc = R["escape"]["result"]
cases = {c["name"]: c for c in esc["cases"]}
bench = R["bench"]["result"]
native = R["native_baseline"]
hosts = [R[k]["host"] for k in ["demo", "escape", "isolation-write", "isolation-read", "bench"]]
setup_ms = statistics.median(h["run_setup_us"] for h in hosts) / 1000
inst_ms = statistics.median(h["instantiate_us"] for h in hosts) / 1000
tear = sorted(h["teardown_us"] / 1000 for h in hosts)
demo_steps = R["demo"]["result"]["steps"]
fork_n = cases["fork_bomb_contained"]["evidence"]["stdout"].split()[1]


def ok(name):
    return '<font color="#1e7b34"><b>blocked</b></font>' if cases[name]["pass"] else '<font color="#b00020"><b>FAILED</b></font>'


s = []
s += [P("Sandbox runs: is it feasible?", h1),
      P("Feasibility report · 2026-11-26 · Prototype: "
        + C("prototypes/sandbox-runs/") + " on branch " + C("claude/calimero-sandbox-support-cyuqc1")
        + " (calimero-network/core)", sub)]

box = Table([[P(
    "<b>Verdict: feasible, with one condition.</b> A WASM driver on wasmtime 51 (core's version) ran a "
    "shell, Python and gcc in an isolated sandbox through a four-function interface. Every one of "
    f"<b>{esc['total']} escape attempts was blocked</b>, under concurrent load as well. The cost is "
    f"<b>about {bench['exec_true_median_ms'] - native['true_median_us'] / 1000:.0f} ms per command</b> over a native spawn and "
    f"<b>{setup_ms:.1f} ms</b> to set up a run. "
    "<b>The condition:</b> a namespace sandbox still shares the job node's kernel. That is acceptable only "
    "because a job node holds no cluster key, and only when runs that share a job node trust each other "
    "(for example, one org per node). Mixing tenants on a node needs a second boundary: gVisor or a TD per "
    "run. That second boundary is the next spike, and it could not be tested in this environment (Section 6).",
    verdict)]], colWidths=[W])
box.setStyle(TableStyle([
    ("BOX", (0, 0), (-1, -1), 1, ACCENT), ("BACKGROUND", (0, 0), (-1, -1), colors.HexColor("#f2f6fb")),
    ("LEFTPADDING", (0, 0), (-1, -1), 9), ("RIGHTPADDING", (0, 0), (-1, -1), 9),
    ("TOPPADDING", (0, 0), (-1, -1), 8), ("BOTTOMPADDING", (0, 0), (-1, -1), 8)]))
s += [box, Spacer(1, 6)]

s += [P("1. The questions", h2)]
s += B([
    "<b>Plumbing.</b> Can a " + C("calimero:app") + "-style component drive real Linux processes through a host "
    "interface that fits on top of the compute-action design?",
    "<b>Isolation.</b> Can the sandbox be built inside a VM <i>without nested virtualization</i>, which is the "
    "situation inside a TDX guest, and stop the attacks that matter?",
    "<b>Host safety.</b> Can the host read and write the sandbox's files for it without being tricked into "
    "touching its own?",
    "<b>Cost.</b> Is the per-run and per-command overhead small enough for a job node?",
])

s += [P("2. What was built", h2),
      P("About 1,370 lines of Rust and WIT, standalone (not in core's workspace). The driver is the part an app "
        "author writes; the host and init are what a job node would run."), ]

# Architecture drawing
d = Drawing(W, 150)


def boxd(x, y, w, h, title, lines, fill):
    d.add(Rect(x, y, w, h, fillColor=fill, strokeColor=ACCENT, strokeWidth=0.8, rx=3, ry=3))
    d.add(String(x + 6, y + h - 12, title, fontName="Helvetica-Bold", fontSize=8.5, fillColor=colors.black))
    for i, ln in enumerate(lines):
        d.add(String(x + 6, y + h - 24 - i * 10, ln, fontName="Helvetica", fontSize=7.3, fillColor=MUTED))


def arrow(x1, y1, x2, y2, label=""):
    d.add(Line(x1, y1, x2, y2, strokeColor=ACCENT, strokeWidth=0.8))
    d.add(Polygon([x2, y2, x2 - 3, y2 + 6, x2 + 3, y2 + 6] if y2 < y1 else [x2, y2, x2 - 6, y2 - 3, x2 - 6, y2 + 3],
                  fillColor=ACCENT, strokeColor=ACCENT))
    if label:
        d.add(String((x1 + x2) / 2 + 5, (y1 + y2) / 2 - 3, label, fontName="Courier", fontSize=7, fillColor=ACCENT))


bw = W * 0.30
boxd(0, 95, bw, 52, "driver.wasm (app code)", ["world calimero:sandbox-proto/driver", "wasmtime 51, WASI without fs or net",
                                                "exports run(task, args)"], colors.HexColor("#eef4ea"))
boxd(0, 5, bw, 66, "host (job node)", ["per run: tmpfs /workspace (64 MiB),", "cgroups: memory, pids, cpu",
                                        "workspace I/O via openat2", "timeouts, output caps, kill-all"], colors.white)
arrow(bw / 2, 95, bw / 2, 71, "sandbox.*")
bx = W * 0.40
boxd(bx, 5, W - bx, 142, "one exec = a fresh sandbox (init)", [
    "1  enter cgroups; unshare mount, pid, net, ipc, uts, cgroup ns",
    "2  fork: PID 1 of the new PID namespace",
    "3  new root on tmpfs: /usr read-only (the 'image'),",
    "    minimal /etc and /dev, /proc, /tmp, /workspace (rw)",
    "4  pivot_root, detach the old root, root fs read-only",
    "5  drop the bounding set, become nobody, no_new_privs,",
    "    rlimits, seccomp deny-list (+ clone3 -> ENOSYS)",
    "6  fork the command; reap; exiting tears down the namespace",
    "",
    "   network: loopback only    env: PATH, HOME, LANG + driver's",
], colors.HexColor("#f6f3ec"))
arrow(bw, 38, bx, 38)
s += [d, Spacer(1, 4)]

s += [P("2.1 The interface", h3),
      P(C("exec(argv, env, cwd, stdin, timeout-ms) -> {exit-code, stdout, stderr, truncated, timed-out, wall-us}") + ", "
        + C("write(path, bytes)") + ", " + C("read(path, max)") + ", " + C("collect(path) -> sha256") + ". "
        "These four were enough for every task. A driver chains them: write the inputs, run a build, read the "
        "results, collect the outputs. In core, " + C("collect") + " becomes a sealed output file and the "
        "driver becomes the compute component, with " + C("sandbox") + " imported next to " + C("files") +
        " and " + C("job") + "."),
      ]

s += [P("2.2 The environment, and what it stands in for", h3),
      table([
          ["Here", "Real job node", "Consequence"],
          ["Firecracker microVM, kernel 6.18, 4 vCPU, 15 GiB", "Intel TDX guest (a TD)", "Same constraint: no /dev/kvm and no VMX flag, so no nested microVMs. The sandbox has to work inside the guest kernel, and it does."],
          ["Root in the VM, cgroup v1", "Root in the TD, cgroup v2 likely", "v2 is a straight port (memory.max, pids.max, cpu.max, cgroup.kill)."],
          ["Host's /usr bound read-only", "App's rootfs image, pinned by digest", "Same mount, different source. Not exercised: image fetch, cache, verification."],
          ["No attestation", "Attested before it gets inputs", "Unchanged from compute actions; not part of this question."],
      ], [0.27 * W, 0.23 * W, 0.50 * W]),
      Spacer(1, 4)]

# ---- results ---------------------------------------------------------------------------
s += [P("3. Results", h2), P("3.1 It does the job", h3),
      table([["Step (driver)", "Exit", "Wall", "Output"]] + [
          [st["step"], str(st["r"]["exit"]), f'{st["r"]["wall_ms"]:.0f} ms', st["r"]["stdout"].split("\n")[0][:60]]
          for st in demo_steps
      ] + [["read report.json, collect it", "—", "—", "sha256 " + R["demo"]["result"]["collected_output_sha256"][:16] + "…"]],
          [0.30 * W, 0.08 * W, 0.12 * W, 0.50 * W]),
      Spacer(1, 4)]

layer = {
    "host_files_hidden": "mount ns + pivot_root, old root detached",
    "rootfs_read_only": "read-only bind mounts, read-only root",
    "unprivileged": "privilege drop + seccomp",
    "setuid_binaries_inert": "nosuid + no_new_privs",
    "process_isolation": "PID namespace (4 processes visible)",
    "no_network": "network ns, loopback only (ENETUNREACH)",
    "namespace_and_mount_syscalls": "seccomp (EPERM)",
    "clone_namespace_flags": "seccomp on clone flags; clone3 → ENOSYS",
    "kernel_and_proc_knobs": "not root: kallsyms zeroed, dmesg EPERM",
    "cgroup_view": "cgroup ns (sees \"/\"), no /sys",
    "env_clean": "env cleared (HOME LANG PATH PWD)",
    "fork_bomb_contained": f"pids cgroup: {fork_n} forks, then EAGAIN",
    "memory_capped": "memory cgroup: OOM kill (exit 137)",
    "cpu_timeout": "host deadline + cgroup kill at 1.5 s",
    "orphans_reaped": "PID 1 exit kills the namespace; cgroup emptied",
    "workspace_quota": "tmpfs size: stops at 64 MiB",
    "output_capped": "host keeps 1 MiB, flags truncated",
    "host_read_symlink_attack": "openat2 RESOLVE_NO_SYMLINKS (ELOOP)",
    "host_write_symlink_attack": "openat2 RESOLVE_NO_SYMLINKS (ELOOP)",
    "host_fifo_trap": "O_NONBLOCK + regular-file check",
    "host_path_traversal": "openat2 RESOLVE_BENEATH (EXDEV)",
}
s += [P(f"3.2 Escape suite: {esc['passed']}/{esc['total']} blocked", h3),
      P("Each case runs untrusted commands inside the sandbox, or plants something and then asks the host to act on "
        "it, and checks what actually happened. The last four target the host's own file I/O."),
      table([["Attack", "Result", "Stopped by"]] + [
          [c["attack"], ok(c["name"]), layer[c["name"]]] for c in esc["cases"]
      ], [0.44 * W, 0.11 * W, 0.45 * W], zebra=True),
      Spacer(1, 4)]

s += [KeepTogether([P("3.3 The suite has teeth", h3),
      P("Following core's “watch it go red” rule, two control modes turn off one layer each, and the suite "
        "catches both:"),
      table([
          ["Mode", "Escape suite", "Cases that go red"],
          ["full", f"{esc['passed']}/{esc['total']}", "none"],
          ["no seccomp", "18/21", "unprivileged, namespace_and_mount_syscalls, clone_namespace_flags"],
          ["root (no drop, no seccomp)", "17/21", "the three above + kernel_and_proc_knobs"],
      ], [0.25 * W, 0.15 * W, 0.60 * W]),
      Spacer(1, 3),
      P("<b>What the controls show:</b> without seccomp, a process running as " + C("nobody") + " can still create "
        "a user namespace (" + C("unshare --user") + " succeeds). That is the classic way in to kernel privilege "
        "escalation. So seccomp is a required layer, not belt-and-braces.")])]

s += [P("3.4 Concurrency and cleanup", h3)]
s += B([
    "4 escape suites (each with a fork bomb, a 1 GiB allocation and a CPU spin) ran <b>in parallel with 8 demo "
    "runs</b>: all 4 scored 21/21 and all 8 demos succeeded with identical output hashes, in 4.3\u20134.5 s wall.",
    "A second run sees nothing of the first: its " + C("/workspace") + " and " + C("/tmp") + " are empty.",
    "After every run: 0 run cgroups, 0 run directories and 0 workspace mounts left on the host.",
])

s += [P("3.5 Cost", h3),
      table([
          ["Measure", "Sandbox", "Native", "Note"],
          ["Spawn " + C("true") + " (median of 21)", f'{bench["exec_true_median_ms"]:.1f} ms', f'{native["true_median_us"] / 1000:.1f} ms',
           "Fixed cost per exec: namespaces, mounts, pivot, seccomp. 6–9 ms across runs."],
          [C("python3 -c pass") + " (median of 11)", f'{bench["exec_python_median_ms"]:.1f} ms', f'{native["python_median_us"] / 1000:.1f} ms', "The same fixed cost on top of real work."],
          ["Run setup (tmpfs, 3 cgroups)", f"{setup_ms:.2f} ms", "—", "Median across the 5 runs."],
          ["Run teardown", f"{tear[0]:.1f}–{tear[-1]:.0f} ms", "—", "Kill-all, cgroup rmdir, unmount."],
          ["Driver instantiate", f"{inst_ms:.2f} ms", "—", "Per run. Compiling the 225 KB component took 80–115 ms once (core caches by hash)."],
          ["1 MiB through stdin/stdout", f'{bench["cat_1mib_ms"]:.0f} ms', "—", "Including the exec."],
          ["Host write + read 4 MiB", f'{bench["host_write_read_4mib_ms"]:.0f} ms', "—", "openat2 path, through WIT lists."],
      ], [0.30 * W, 0.12 * W, 0.10 * W, 0.48 * W]),
      Spacer(1, 3),
      P("For runs that take seconds to minutes (builds, tests, data jobs), a few milliseconds per command doesn't "
        "matter. Only a driver that issues thousands of tiny commands would notice, and it can batch them into one "
        "shell script."),
      ]

# ---- findings --------------------------------------------------------------------------
s += [P("4. What we learned", h2)]
s += B([
    "<b>The WASM side is the easy part.</b> The interface binds cleanly with " + C("bindgen!") + " on wasmtime 51 "
    "and " + C("wit-bindgen") + " 0.63, the same versions core uses. The driver needs no WASI filesystem or sockets. "
    "It fits the compute world as one more import.",
    "<b>Namespaces + pivot_root do most of the isolation.</b> Even in the “root” control, the host's "
    "files, processes and network stayed out of reach. The privilege drop and seccomp protect the kernel, "
    "which is what's left exposed.",
    "<b>Host-side I/O is the subtle part.</b> The host acts as root on paths the sandbox controls. Symlinks, "
    "" + C("..") + ", " + C("/proc/self/root") + " and FIFOs are all live attacks. " + C("openat2") + " with "
    + C("RESOLVE_BENEATH | NO_SYMLINKS | NO_MAGICLINKS | NO_XDEV") + " plus an " + C("O_NONBLOCK") + " "
    "regular-file check closes them in the kernel, with no check-then-use race. The real spec should require this.",
    "<b>Results are not deterministic.</b> Sandboxed tools read clocks and randomness, and the output differs "
    "between attempts. Unlike compute runs, a restart is a new attempt, and the first attempt to finish is the "
    "result. This matches the earlier proposal.",
    "<b>Plain cgroup v1 works, but v2 is better.</b> Killing a run took a read-and-SIGKILL loop. v2's "
    + C("cgroup.kill") + " does it in one atomic write.",
])

s += [KeepTogether([P("5. Residual risk: the shared kernel", h2),
      P("Every layer here is enforced by the job node's own kernel. A kernel exploit reachable through the "
        "syscalls the deny-list still allows would give the attacker the job node. What that exposes:")]),
      table([
          ["On the job node", "Exposed by a kernel escape?"],
          ["Cluster keys, KMS keys, other apps' state", "<b>No.</b> Job nodes never hold them (compute-action design)."],
          ["Other runs on the same job node, while they run", "<b>Yes.</b> Their inputs' plaintext and their outputs."],
          ["The job node's attestation identity", "Yes, until it is recycled."],
      ], [0.45 * W, 0.55 * W]),
      Spacer(1, 4),
      P("So the deployment rule decides it:"),
      ]
s += B([
    "<b>Single-tenant job nodes</b> (one org, or one app owner, per node): the namespace sandbox as built is enough. "
    "An escape reaches only data that tenant could already see.",
    "<b>Multi-tenant job nodes</b>: add a second boundary. Either <b>gVisor</b> (a user-space kernel; the systrap "
    "platform needs no virtualization), which narrows what the sandbox can ask of the host kernel to a small set of syscalls, or "
    "<b>one TD per run</b>, which is the strongest boundary but adds TD boot time per run.",
    "In both cases, replace the deny-list with an <b>allow-list</b> seccomp profile derived from the "
    "toolchains we support.",
])

s += [P("6. Not tested here", h2),
      table([
          ["Gap", "Why", "How to close"],
          ["gVisor inside a VM with no nested virtualization", "Downloading and running the runsc binary was not permitted in this session", "Spike 2 on a real TD job node: syscall coverage for git/Python/Node/Rust, and overhead"],
          ["Inside an actual TD", "No TDX hardware here", "Same spike. Expect the same behavior: TDX changes memory protection, not namespaces or seccomp"],
          ["TD-per-run boot cost", "No TDX", "Measure TD cold start on the target cloud; decide the multi-tenant option from that number"],
          ["cgroup v2, OCI image pinning", "v1 host; host /usr as the image", "Port to v2 + cgroup.kill; mount a verified rootfs by digest"],
          ["Egress", "Out of scope", "Network namespace + an HTTPS proxy that applies the outbound gates"],
          ["Kernel exploit resistance", "Can't be shown by tests", "Allow-list seccomp, pinned kernel, gVisor; external review"],
      ], [0.28 * W, 0.30 * W, 0.42 * W]),
      Spacer(1, 4)]

s += [P("7. Recommendation", h2)]
s += B([
    "<b>Go</b> with the compute-action-based design from the earlier proposal (driver component, sealed files in "
    "and out, result method, feature gate), with these changes from the prototype:",
])
sub_b = ParagraphStyle("bl2", parent=bullet, leftIndent=26, bulletIndent=14)
s += [Paragraph(t, sub_b, bulletText="–") for t in [
    "the host's workspace I/O spec requires " + C("openat2") + " resolve flags and regular files only;",
    "seccomp is mandatory, as an allow-list; cgroup v2 with " + C("cgroup.kill") + ";",
    "tenancy is explicit: sandbox job pools are single-tenant until Spike 2 picks gVisor or TD-per-run.",
]]
s += B([
    "<b>Next:</b> Spike 2 on a real TD job node (gVisor and TD cold start). Then wire " + C("sandbox") +
    " into " + C("calimero-app-engine-wasmtime") + "'s compute world behind a " + C("sandbox_runs") + " feature.",
])

s += [Spacer(1, 6), P("<b>Reproduce:</b> " + C("cd prototypes/sandbox-runs && ./run.sh results.json") +
                      " (root, Linux, Rust with " + C("wasm32-wasip2") + "). Controls: " + C("SANDBOX_PROTO_CONTROL=no-seccomp|root") +
                      ". The run this report quotes is " + C("results/firecracker-vm-2026-11-26.json") + ".", small)]

doc = SimpleDocTemplate(OUT, pagesize=A4, leftMargin=18 * mm, rightMargin=18 * mm,
                        topMargin=16 * mm, bottomMargin=18 * mm,
                        title="Sandbox runs: feasibility report", author="Calimero Core")
doc.build(s, onFirstPage=footer, onLaterPages=footer)
