# indexed-issue-tracker

An issue tracker on the component SDK (`calimero-sdk`) whose list views are
index seeks instead of scans over every issue. The v1 version was the example
for `IndexedMap` + `#[derive(app::Indexed)]`; here a `Table` declares the
lookups:

```rust
#[derive(Record)]
#[table(
    key = "id",
    index(status_created = ["status", "created_at"]),
    index(by_assignee = ["assignee"]),
    count(by_status = ["status"]),
)]
pub struct Issue { id, title, status, assignee: Option<String>, created_at: u64 }

#[derive(Record)]
#[table(
    key = ["issue_id", "label"],
    index(by_label = ["label", "issue_id"]),
    index(by_issue = ["issue_id", "label"]),
)]
pub struct Label { issue_id, label }   // one row per label of an issue
```

Methods: `open { id, title, labels, createdAt }` (status `open`), `setStatus { id, status }`
(`open`, `in_progress`, `closed`), `assign { id, assignee }`, `get { id }` and the lists
below, plus `issueCount`. Errors: `Exists`, `NotFound` (each with its `id`),
`UnknownStatus { status }`. Events, to the organization: `Opened { id }`, `Changed { id }`.

| method | read | cost |
|---|---|---|
| `listByStatus` | `find("statusCreated", [status], reverse)` | the rows returned (plus `offset`) |
| `listCreatedBetween` | `find("statusCreated", [status], from..to)` | the rows in range |
| `listByLabel` | `labels.find("byLabel", [label])` | the issues with that label |
| `listByAssignee` | `find("byAssignee", [who])` | that person's issues |
| `statusCounts` | `count("byStatus", [status])` | one read per status, no issue loaded |

Changing an indexed field goes through `Table::update`, which keeps every index
and count in step. The indexes are part of the app's state, replicated with it,
so no replica ever rebuilds them (v1's indexes were node-local and rebuilt after
a sync). Any org member files, moves and assigns issues; guests get `Forbidden`.
`createdAt` and the counts are decimal strings.

```bash
cargo test --manifest-path apps/indexed-issue-tracker/Cargo.toml
```

`tests/scenarios.rs` runs `scenarios/indexed-issue-tracker.json`, the former
merobox workflow, in the harness; the cluster runs the same file.
