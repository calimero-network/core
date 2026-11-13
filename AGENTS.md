# Calimero Core - AI Agent Guidance

A confidential cloud for collaborative apps. Apps are `calimero:app` WASM components, written in Rust with `calimero-sdk` and built with `mero-app-build` (the engine also takes componentize-js components; core ships no JavaScript SDK); each app instance is replicated by its own Raft log across attested TDX nodes (`merod`, inside it `calimero-node`), which clients reach through a sealed channel after verifying the node's attestation. `meroctl` is the command line. Apps run on wasmtime; SDK 0.12 apps (wasmer) are gone.

## Where to read

`docs/` explains why the system works as it does and how the crates add up; a directory's `AGENTS.md` explains that one unit.

- **Standards**: before writing or reviewing code, read [CODING_STANDARDS.md](CODING_STANDARDS.md).
- **Intent**: before changing a flow that spans crates, read the [protocol overview](docs/src/content/docs/protocol/overview.mdx) and the chapters it names, all under `docs/src/content/docs/protocol/`.
- **Directory**: before working in a crate, app or tool, read the `AGENTS.md` in its directory (the `CLAUDE.md` beside it is a symlink); [crates/AGENTS.md](crates/AGENTS.md) indexes the crates and [architecture](docs/src/content/docs/contribute/architecture.mdx) shows how they connect.
- **Glossary**: when a term is unclear, read the [glossary](docs/src/content/docs/protocol/glossary.mdx); `meroctl`'s names map to these: namespace = organization, group = team, context = app instance, application = signed package (see [meroctl](docs/src/content/docs/operate/meroctl.mdx)).
- **Nodes**: before running `merod` by hand, read [Run local nodes](docs/src/content/docs/contribute/development.mdx#run-local-nodes); every flag is in the [merod reference](docs/src/content/docs/operate/merod.mdx).
- **Tests**: before writing a test or scenario file, reading node logs, or debugging a CI failure, read [Testing strategy](docs/src/content/docs/contribute/testing.mdx).
- **Releasing**: before committing, titling a pull request, or releasing, read [RELEASING.md](RELEASING.md): the PR title is the release commit, and the same rules hold in every repository that ships releases.
- **Issues**: before filing an engineering issue, fill in the [`technical_issue`](.github/ISSUE_TEMPLATE/technical_issue.md) template.

## Testing

- **Reproduce first**: write the failing test at the layer a user hits the bug and watch it go red before changing code.
- **Blind spot**: scenarios and the process suite run one build on fresh state, so they never test backward compatibility; an on-disk or wire format change needs its own migration test.
- **Old shapes**: a stored or replicated shape (row, log entry, snapshot, sealed file record) changes by raising its format version (`STORE_FORMAT`, `SNAPSHOT_FORMAT`, `ENTRY_FORMAT`) and keeping a reader for the old one, with a test that reads data written in the old shape ([Compatibility](docs/src/content/docs/operate/compatibility.mdx), "Changing a stored format").
- **Feature gate**: a new command or field ships with a feature gate. Old and new nodes share one log through a rollout, and a node skips an entry it cannot decode, so a new log command, or a new field that changes what one does, is a `Feature` in `system::cluster_version::FEATURES` (with the release it ships in) and is refused `409 FEATURE_NOT_ACTIVE` below the cluster version ([The cluster version](docs/src/content/docs/protocol/upgrades.mdx#the-cluster-version)).
- **Upgrade path**: CI's `Upgrade path` job (required through `Rust`) starts a cluster on the release in `.github/upgrade-from`, writes data, rolls it onto the PR's `merod` as mdma rolls a Cloud cluster and reads it back ([Upgrade path](docs/src/content/docs/contribute/testing.mdx#upgrade-path)); a new kind of stored data gets a case in `crates/node/tests/fault/upgrade_cases/`.
  A change that turns it red ships a reader or migration for the old data, or declares a hard cutover (`hard-cutover:` in `.github/upgrade-from`, the CHANGELOG, mdma's `release_compat` CUTOVERS); never silently.

## Definition of Done

- **Gates**: run `./scripts/check-like-ci.py` before opening a PR; it reads `.github/workflows/ci-checks.yml` and runs every job the required `Rust` check waits on (`--list` shows them, `--only <name>` runs a subset).
  Run clippy by hand with `-D warnings`, as CI does; `-A warnings` allows every lint and can only pass.
- **Compatibility**: a route or JSON field of the API is removed only after its `[[deprecated]]` entry in `crates/node/api-deprecations.toml` reaches `remove_after` (3 releases), and a WIT change bumps `calimero:app`'s version (`Compatibility` in ci-checks); a PR whose `Door:` is one-way names its `Reader:` or declares `Hard cutover:` with a CHANGELOG line, the `[[cutover]]` entry and mdma's `CUTOVERS` line ([Compatibility](docs/src/content/docs/operate/compatibility.mdx)).
- **Proof**: a bug fix's PR description shows the reproduction (test, command or scenario file) and its before and after output; the reproducing test stays as the regression test.
- **Upgrade case**: a PR that adds user-visible functionality adds a `DeepCase` for it in `crates/node/tests/fault/upgrade_cases/` (`since` = the release it ships in) and pastes the regenerated coverage matrix into [Deep cases](docs/src/content/docs/contribute/testing.mdx#deep-cases-every-functionality); what cannot be tested there yet goes in `NOT_COVERED` with the reason.
- **Docs**: update the README, `AGENTS.md`, crate docs or `docs/` page the change makes stale, no later than one day after merge.
- **Merge**: a PR merges when CI is green, every meroreviewer and Cursor Bugbot thread is fixed or answered and resolved, and branch protection's human review is approved.
  The `resolve-bot-review-comments`, `babysit` and `code-review` skills drive that loop.
