---
name: Technical issue
about: Report a defect or engineering issue with a clear summary, impact, repro, and resolution criteria
title: ''
labels: ''
assignees: ''
---

## Summary

A single-paragraph statement of the problem: what is wrong, and where (crate, module, or flow). State the observed behavior, not a proposed fix.

## Impact

Who or what is affected and how badly. Cover, where relevant:

- Severity (data loss / security / correctness / performance / cosmetic) and who hits it.
- Blast radius: does it affect one node, a group, or every deployment?
- Real-world consequence: a concrete scenario where this bites (e.g. "a member who leaves still decrypts new messages").

## Steps to reproduce

Numbered, minimal steps someone else can follow to see the issue.

1. ...
2. ...
3. See error / wrong result

Include the actual vs expected result, and attach logs, a failing test, or a merobox scenario if you have one.

## Criteria for resolving

A checklist that objectively decides when this is fixed. Behavior first, then verification.

- [ ] <the specific behavior that must hold once fixed>
- [ ] Regression test covering the case above (unit test or merobox scenario)
- [ ] `cargo fmt --check`, `cargo clippy`, and `cargo test` pass
