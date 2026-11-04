# compute-redact

Redacts a CSV file on compute runs, on the component SDK (`calimero-app-sdk`, `wasm32-wasip2`).
One crate builds two components: `app.wasm` (state and calls) and `compute.wasm` (the rules engine, which is all a job node receives).

## Roles

- The organization's admin or owner sets the policy once with `setPolicy {policy}`.
- A member uploads a CSV as a sealed file and calls `submit {file, rowsPerPart?}`; the job copies the policy, so a later `setPolicy` does not change it.
- `job {id}` and `jobs` show a job to its submitter and to admins; the view holds the policy's SHA-256, never the salt.
- Delivering a finished job's output outside the organization comes with `files::publish`, and is not part of this app yet.

## Policy

```json
{ "salt": "a secret", "columns": { "email": "mask", "name": "pseudonym", "ssn": "drop", "note": "scan" } }
```

| Rule | Cell becomes |
| ---- | ------------ |
| `drop` | empty |
| `mask` | a hint: `jane.doe@acme.com` is `j***@***.com`, digits keep the last two, other text keeps its first character |
| `pseudonym` | `P-` and the first 12 hex digits of HMAC-SHA256 (salt, value), the same for the same value in every row and part |
| `scan` | the text with emails, phone numbers and card-like numbers replaced by `[EMAIL]`, `[PHONE]`, `[CARD]` |

Columns the policy does not name pass through.
`scan` uses the `regex` crate.

## How a job runs

1. `submit` queues a `preview` run: the first 500 rows (at most 48 KiB of JSON, under a result's 64 KiB), redacted, and the file's row count.
2. The row count plans the parts, `rowsPerPart` rows each (50,000 by default, raised so a file has at most 256 parts).
3. Each part is one `redact` run writing one output file, header first; `on_run` stores its `BlobRef`, which keeps the file.
4. At most 4 runs are active in the app; the rest wait, oldest job first.
5. A part's progress (`N of M rows, K cells redacted`) is reported with `compute::progress`.

The output is the same bytes for the same input and policy, so a retried run gives the same file.
Every part reads the file from its start and skips to its rows.

## Build and test

```bash
cargo test --manifest-path apps/compute-redact/Cargo.toml
mero-app-build apps/compute-redact --out app.wasm --compute-out compute.wasm
```

`tests/harness.rs` runs the real component for the roles.
The harness runs no compute, so the queue and the runs' outcomes are tested in `src/lib.rs` with a fake starter, and the rules in `src/rules.rs`.
