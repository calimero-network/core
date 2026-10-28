# Calimero Core - AI Agent Guidance

A confidential cloud for collaborative apps (release-v2). Apps are written in Rust or JavaScript and compiled to WASM; each app instance is replicated by its own Raft log across attested TDX nodes (`merod`, inside it `calimero-node`), which clients reach through a sealed channel after verifying the node's attestation. `meroctl` is the command line. v1's peer-to-peer node is deleted; the SDK and WASM runtime are unchanged.

## Where to read

`docs/` explains why the system works as it does and how the crates add up; a directory's `AGENTS.md` explains that one unit.

- **Standards**: before writing or reviewing code, read [CODING_STANDARDS.md](CODING_STANDARDS.md).
- **Intent**: before changing a flow that spans crates, read the [protocol overview](docs/src/content/docs/protocol/overview.mdx) and the chapters it names, all under `docs/src/content/docs/protocol/`.
- **Directory**: before working in a crate, app or tool, read the `AGENTS.md` in its directory (the `CLAUDE.md` beside it is a symlink); [crates/AGENTS.md](crates/AGENTS.md) indexes the crates and [architecture](docs/src/content/docs/contribute/architecture.mdx) shows how they connect.
- **Glossary**: when a term is unclear, read the [glossary](docs/src/content/docs/protocol/glossary.mdx); v1's names map to release-v2 as namespace = organization, group = team, context = app instance, application = signed package (see [meroctl](docs/src/content/docs/operate/meroctl.mdx)).
- **Nodes**: before running `merod` by hand, read [Run local nodes](docs/src/content/docs/contribute/development.mdx#run-local-nodes); every flag is in the [merod reference](docs/src/content/docs/operate/merod.mdx).
- **Tests**: before writing a test or merobox scenario, reading node logs, or debugging a CI failure, read [Testing strategy](docs/src/content/docs/contribute/testing.mdx).
- **Issues**: before filing an engineering issue, fill in the [`technical_issue`](.github/ISSUE_TEMPLATE/technical_issue.md) template.

## Testing

- **Reproduce first**: write the failing test at the layer a user hits the bug and watch it go red before changing code.
- **Blind spot**: merobox scenarios run one build on fresh state, so they never test backward compatibility; an on-disk or wire format change needs its own migration test.

## Definition of Done

- **Gates**: run `./scripts/check-like-ci.py` before opening a PR; it reads `.github/workflows/ci-checks.yml` and runs every job the required `Rust` check waits on (`--list` shows them, `--only <name>` runs a subset).
  Run clippy by hand with `-D warnings`, as CI does; `-A warnings` allows every lint and can only pass.
- **Proof**: a bug fix's PR description shows the reproduction (test, command or merobox scenario) and its before and after output; the reproducing test stays as the regression test.
- **Docs**: update the README, `AGENTS.md`, crate docs or `docs/` page the change makes stale, no later than one day after merge.
- **Merge**: a PR merges when CI is green, every meroreviewer and Cursor Bugbot thread is fixed or answered and resolved, and branch protection's human review is approved.
  The `resolve-bot-review-comments`, `babysit` and `code-review` skills drive that loop.
