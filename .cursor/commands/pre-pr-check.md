# Pre-PR Check

Run the Definition of Done checks before creating a PR. The full Definition of Done is in the root `AGENTS.md`.

**Instructions:**

1. Run `./scripts/check-like-ci.py` and report results; it runs every job CI's required `Rust` check waits on, read from `.github/workflows/ci-checks.yml`.
2. For any failure, fix the issues and re-run (`./scripts/check-like-ci.py --only <name>`) until all pass
3. Remind that documentation must be updated (README, AGENTS.md, crate docs) if the change warrants it
