# Replay fixtures

One directory per older release the replay keeps (`upgrade_replay.rs`), written by
`crates/node/tests/fault/upgrade_fixture.py`. The pinned release is not here: the
`Upgrade path` job records it on every run (`scripts/upgrade-path.sh`), with that
release's own binaries. A hard cutover drops the fixtures of
the releases before it, since this tree no longer reads their data: 1.0.0-rc.20
(every app in an organization) dropped 1.0.0-rc.15's, and 1.0.0-rc.33 (an
instance's access is its grants, #5194; public links are grants, #5195) dropped
every one before it. A release that is itself the cutover writes data this tree
reads, so its fixture stays until the next cutover.

Held now:

- `1.0.0-rc.33/`: the cutover release itself.
- `1.0.0-rc.34/`
