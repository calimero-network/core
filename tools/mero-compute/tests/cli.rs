//! `mero-compute` on the cluster tests' compute fixture, through the real binary.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use calimero_app_build::build_in_repo_cache;
use rand_chacha::rand_core::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;

const PIECE: usize = 64 * 1024; // what the fixture reads and keys at a time
const SEED: [u8; 32] = [9; 32];

struct Built {
    compute: PathBuf,
    app: PathBuf,
}

/// The fixture's two components, built once for every test.
fn built() -> &'static Built {
    static BUILT: OnceLock<Built> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/v2-fixtures/compute");
        let both = build_in_repo_cache(&dir).expect("build the compute fixture");
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("components");
        std::fs::create_dir_all(&out).expect("components dir");
        let (compute, app) = (out.join("compute.wasm"), out.join("app.wasm"));
        // nextest runs each test in its own process, so every process builds
        // and writes these: write aside, then rename into place, so no test
        // ever reads one another process has just truncated.
        let put = |path: &Path, bytes: &[u8]| {
            let aside = path.with_extension(format!("wasm.{}", std::process::id()));
            std::fs::write(&aside, bytes).expect("write a component");
            std::fs::rename(&aside, path).expect("move a component into place");
        };
        put(
            &compute,
            &both.compute.expect("a compute component").component,
        );
        put(&app, &both.app.component);
        Built { compute, app }
    })
}

fn tool(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mero-compute"))
        .arg("run")
        .args(args)
        .output()
        .expect("run mero-compute")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The output the `digest` fixture writes: the input XOR the run's seeded random stream.
fn expected_output(plain: &[u8], seed: [u8; 32]) -> Vec<u8> {
    let mut rng = ChaCha20Rng::from_seed(seed);
    let mut out = Vec::with_capacity(plain.len());
    for piece in plain.chunks(PIECE) {
        let mut keystream = vec![0u8; piece.len()];
        rng.fill_bytes(&mut keystream);
        out.extend(piece.iter().zip(&keystream).map(|(p, k)| p ^ k));
    }
    out
}

fn input_file(dir: &Path, len: usize) -> (PathBuf, Vec<u8>) {
    let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let path = dir.join("input.bin");
    std::fs::write(&path, &plain).expect("write input");
    (path, plain)
}

#[test]
fn digest_writes_the_bytes_a_job_node_writes() {
    let work = tempfile::tempdir().expect("tempdir");
    let (input, plain) = input_file(work.path(), 3 * PIECE + 1000);
    let out = work.path().join("out");
    let run = tool(&[
        built().compute.to_str().unwrap(),
        "--method",
        "digest",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--seed",
        &hex_of(&SEED),
    ]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert_eq!(
        std::fs::read(out.join("output-0")).expect("output-0"),
        expected_output(&plain, SEED)
    );
    let shown = text(&run.stdout);
    assert!(shown.contains("progress: 100% hashing"), "{shown}");
    assert!(shown.contains("output-0"), "{shown}");
    assert!(shown.contains("cpu seconds"), "{shown}");
    assert!(shown.contains("fuel"), "{shown}");
    // The result is a JSON list of each input's SHA-256: shown as base64 and pretty.
    assert!(shown.contains("result (base64): "), "{shown}");
    assert!(shown.contains("result: ["), "{shown}");
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn a_run_that_breaks_a_cap_fails_with_its_reason_and_a_non_zero_exit() {
    let work = tempfile::tempdir().expect("tempdir");
    for (method, reason) in [
        ("burn", "fuel"),
        ("hog", "memory"),
        ("spin", "lost"),
        ("big", "trap"),
    ] {
        let out = work.path().join(method);
        let run = tool(&[
            built().compute.to_str().unwrap(),
            "--method",
            method,
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(!run.status.success(), "{method}");
        let err = text(&run.stderr);
        assert!(
            err.contains(&format!("failed: {reason}")),
            "{method}: {err}"
        );
    }
}

#[test]
fn twice_passes_on_a_run_that_is_the_same_both_times() {
    let work = tempfile::tempdir().expect("tempdir");
    let (input, _) = input_file(work.path(), PIECE + 7);
    let out = work.path().join("out");
    let run = tool(&[
        built().compute.to_str().unwrap(),
        "--method",
        "digest",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--twice",
    ]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(text(&run.stdout).contains("both runs are identical"));
    assert!(out.join("output-0").exists());
    let mut left: Vec<_> = std::fs::read_dir(&out)
        .expect("read out")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf8"))
        .collect();
    left.sort();
    assert_eq!(left, ["output-0"], "the second run leaves nothing");
}

#[test]
fn a_stale_output_from_an_earlier_run_is_removed() {
    let work = tempfile::tempdir().expect("tempdir");
    let (input, _) = input_file(work.path(), PIECE + 7);
    let out = work.path().join("out");
    std::fs::create_dir_all(&out).expect("mkdir");
    std::fs::write(out.join("output-5"), b"stale").expect("write");
    let run = tool(&[
        built().compute.to_str().unwrap(),
        "--method",
        "digest",
        "--input",
        input.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(out.join("output-0").exists());
    assert!(!out.join("output-5").exists());
}

#[test]
fn an_app_component_is_refused_with_a_message_that_says_why() {
    let work = tempfile::tempdir().expect("tempdir");
    let run = tool(&[
        built().app.to_str().unwrap(),
        "--method",
        "digest",
        "--out",
        work.path().join("out").to_str().unwrap(),
    ]);
    assert!(!run.status.success());
    let err = text(&run.stderr);
    assert!(err.contains("not a compute component"), "{err}");
}

#[test]
fn a_method_that_is_not_a_compute_method_lists_the_ones_that_are() {
    let work = tempfile::tempdir().expect("tempdir");
    let run = tool(&[
        built().compute.to_str().unwrap(),
        "--method",
        "nope",
        "--out",
        work.path().join("out").to_str().unwrap(),
    ]);
    assert!(!run.status.success());
    let err = text(&run.stderr);
    assert!(err.contains("digest") && err.contains("burn"), "{err}");
}
