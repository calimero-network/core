# Replay fixtures

One directory per release the replay is pinned to (`upgrade_replay.rs`), written by
`crates/node/tests/fault/upgrade_fixture.py`. A hard cutover drops the fixtures of
the releases before it, since this tree no longer reads their data: the last one,
1.0.0-rc.20 (every app in an organization), dropped 1.0.0-rc.15's. The first
release after a cutover records the next fixture here.
