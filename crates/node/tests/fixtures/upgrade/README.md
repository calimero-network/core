# Replay fixtures

One directory per release the replay is pinned to (`upgrade_replay.rs`), written by
`crates/node/tests/fault/upgrade_fixture.py`. A hard cutover drops the fixtures of
the releases before it, since this tree no longer reads their data: 1.0.0-rc.20
(every app in an organization) dropped 1.0.0-rc.15's, and 1.0.0-rc.33 (an
instance's access is its grants, #5194; public links are grants, #5195) dropped
every one before it. The first release after a cutover records the next fixture
here; a release that is itself the cutover writes data this tree reads, so it
records one too.

Held now:

- `1.0.0-rc.33/`: the cutover release itself.
- `1.0.0-rc.34/`
