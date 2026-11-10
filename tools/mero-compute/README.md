# mero-compute

Runs one compute method of a compute component on its own, the way a job node would.

## About

`mero-compute run` loads `compute.wasm`, runs one `#[app::compute]` method on the compute engine a job node uses, and prints the result.
It uses the method's declared fuel and memory caps, the fixed clock and the seeded randomness of a real run, and no network.
Inputs are plain files and outputs are written as plain files, so no cluster, app or sealed file is needed.

It does not build anything.
`mero-app-build` writes the compute component, and the [compute actions guide](../../docs/src/content/docs/build/guides/compute-actions.mdx) shows where it fits.

## Installation

```bash
# Put it on PATH (cargo's bin directory)
scripts/install-mero-compute.sh

# Or run it from the workspace without installing
cargo run -p mero-compute -- run <COMPONENT> --method <METHOD> --out <DIR>
```

## Usage

`apps/compute-redact` redacts a CSV.
This builds both components, then runs the `redact` method on its example file twice and compares the runs:

```bash
mero-app-build apps/compute-redact --out app.wasm --compute-out compute.wasm
mero-compute run compute.wasm --method redact \
  --args @apps/compute-redact/example/redact.json \
  --input apps/compute-redact/example/people.csv \
  --out out --twice
```

The inputs are the files after `--input`, in order, and the run sees them as its `compute::inputs()`.
Each file the run writes lands in `--out` as `output-0`, `output-1` and so on, in the order the run finished them.
Here `out/output-0` is the redacted CSV.

The command prints the progress as the run reports it, the result (base64, and as JSON when it is JSON), each output with its size, and the usage (CPU seconds, GiB-seconds, fuel).
It exits non-zero, naming the reason, when the run does not end done: `fuel`, `memory`, `trap`, `app-error` or `lost`.
A wall clock that runs out is `lost`, as on a job node.

### Flags

| Flag | Meaning |
| --- | --- |
| `<COMPONENT>` | The compute component, `compute.wasm` here, or `compute.component.wasm`, which `mero-app-build` writes beside `--out` when `--compute-out` is not given. An app component is refused. |
| `--method <NAME>` | The compute method, by its ABI name (camelCase). Required. A name that is not a compute method lists the ones that are. |
| `--args <JSON or @FILE>` | The method's arguments as JSON text, or `@` and a file holding it. |
| `--input <FILE>` | An input file. Repeat it for several. |
| `--out <DIR>` | Where the outputs go. Required. |
| `--seed <HEX>` | The randomness seed, 64 hex digits. The default is all zeros. |
| `--twice` | Run again into a scratch directory, delete it, and fail if the result or any output differs. |

## What it does not emulate

- **The app.** There is no state, no call and no result method: `on_run` and the logic that starts the next run are not run.
- **Sealing.** Inputs and outputs are plain files, with no keys, quota or retention.
- **The cluster.** There is no queue, no concurrency limit, no job node attestation, no heartbeats, and no restart on another node.
  A run that would be lost simply fails here.
- **Where the caps come from.** They are the method's own declared caps, not the organization's quotas.

Test the app side with the SDK's `TestApp` harness (`crates/sdk/AGENTS.md`).
