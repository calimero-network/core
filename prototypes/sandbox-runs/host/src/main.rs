//! Prototype host for sandbox runs: loads a driver component (world
//! `calimero:sandbox-proto/driver`) on wasmtime, gives it the `sandbox` interface backed by
//! a namespace sandbox, and runs one task per fresh sandbox.
//!
//!   sandbox-proto-host <driver.wasm> <task>          one task, prints the driver's JSON
//!   sandbox-proto-host <driver.wasm> all <out.json>  demo, escape suite, isolation, bench

mod init;
mod sandbox;

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ path: "../wit", world: "driver" });

use calimero::sandbox_proto::sandbox as sb;

const CONTROLLERS: &[&str] = &["memory", "pids", "cpu"];

struct State {
    run: sandbox::Run,
    wasi: WasiCtx,
    table: ResourceTable,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl sb::Host for State {
    fn exec(&mut self, req: sb::ExecRequest) -> Result<sb::ExecResult, String> {
        let o = self.run.exec(
            &req.argv,
            &req.env,
            req.cwd.as_deref(),
            req.stdin,
            req.timeout_ms,
        )?;
        Ok(sb::ExecResult {
            exit_code: o.exit_code,
            stdout: o.stdout,
            stderr: o.stderr,
            truncated: o.truncated,
            timed_out: o.timed_out,
            wall_us: o.wall_us,
        })
    }
    fn write(&mut self, path: String, bytes: Vec<u8>) -> Result<(), String> {
        self.run.write(&path, &bytes)
    }
    fn read(&mut self, path: String, max: u32) -> Result<Vec<u8>, String> {
        self.run.read(&path, max)
    }
    fn collect(&mut self, path: String) -> Result<String, String> {
        self.run.collect(&path)
    }
}

struct Host {
    engine: Engine,
    component: Component,
    linker: Linker<State>,
}

impl Host {
    fn new(driver: &Path) -> Result<Self> {
        let engine = Engine::new(&Config::new())?;
        let t = Instant::now();
        let component = Component::from_file(&engine, driver)
            .map_err(|e| anyhow!("load {}: {e:?}", driver.display()))?;
        eprintln!("compiled driver in {:?}", t.elapsed());
        let mut linker = Linker::new(&engine);
        // WASI for Rust std inside the driver: no preopens, no env, no args, no sockets.
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        sb::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)?;
        Ok(Self {
            engine,
            component,
            linker,
        })
    }

    /// One task in one fresh sandbox; returns the driver's JSON and host timings.
    fn run_task(&self, task: &str) -> Result<(Value, Value)> {
        let t0 = Instant::now();
        let run = sandbox::Run::create(
            sandbox::Limits::default(),
            Path::new("/run/sandbox-proto-outputs"),
        )?;
        let setup_us = t0.elapsed().as_micros() as u64;
        let wasi = WasiCtxBuilder::new().inherit_stderr().build();
        let mut store = Store::new(
            &self.engine,
            State {
                run,
                wasi,
                table: ResourceTable::new(),
            },
        );
        let t1 = Instant::now();
        let driver = Driver::instantiate(&mut store, &self.component, &self.linker)?;
        let instantiate_us = t1.elapsed().as_micros() as u64;
        let t2 = Instant::now();
        let out = driver.call_run(&mut store, task, &[])?;
        let task_us = t2.elapsed().as_micros() as u64;
        let t3 = Instant::now();
        drop(store); // tears the sandbox down: kill, cgroups, workspace
        let teardown_us = t3.elapsed().as_micros() as u64;
        let out = out.map_err(|e| anyhow!("driver task {task}: {e}"))?;
        let v: Value = serde_json::from_slice(&out).context("driver output is not JSON")?;
        let timings = json!({
            "run_setup_us": setup_us, "instantiate_us": instantiate_us,
            "task_us": task_us, "teardown_us": teardown_us,
        });
        Ok((v, timings))
    }
}

fn native_baseline() -> Value {
    let time = |argv: &[&str], n: usize| -> u64 {
        let mut v: Vec<u64> = (0..n)
            .map(|_| {
                let t = Instant::now();
                let _ = Command::new(argv[0]).args(&argv[1..]).output();
                t.elapsed().as_micros() as u64
            })
            .collect();
        v.sort();
        v[n / 2]
    };
    json!({
        "true_median_us": time(&["true"], 21),
        "python_median_us": time(&["python3", "-c", "pass"], 11),
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(init::INIT_ARG) {
        init::main(&args[2..]);
    }
    let (driver, task) = match &args[1..] {
        [d, t, ..] => (Path::new(d), t.as_str()),
        _ => bail!("usage: sandbox-proto-host <driver.wasm> <task|all> [out.json]"),
    };
    let host = Host::new(driver)?;
    if task != "all" {
        let (v, t) = host.run_task(task)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "result": v, "host": t }))?
        );
        return Ok(());
    }

    let mut report = serde_json::Map::new();
    for task in [
        "demo",
        "escape",
        "isolation-write",
        "isolation-read",
        "bench",
    ] {
        eprintln!("== {task}");
        let (v, t) = host.run_task(task)?;
        report.insert(task.into(), json!({ "result": v, "host": t }));
    }
    report.insert("native_baseline".into(), native_baseline());
    // Leftovers after every run was dropped: there should be none.
    let dirs = |p: &str| {
        std::fs::read_dir(p)
            .map(|d| d.flatten().filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    };
    let leftovers = json!({
        "run_cgroups": CONTROLLERS.iter().map(|c| dirs(&format!("/sys/fs/cgroup/{c}/sandbox-proto"))).sum::<usize>(),
        "run_dirs": dirs("/run/sandbox-proto"),
        "workspace_mounts": std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default().matches("/run/sandbox-proto/").count(),
    });
    report.insert("leftovers".into(), leftovers);
    let out = serde_json::to_string_pretty(&Value::Object(report))?;
    if let Some(path) = args.get(3) {
        std::fs::write(path, &out)?;
        eprintln!("wrote {path}");
    }
    println!("{out}");
    Ok(())
}
