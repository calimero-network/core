# Releasing

mero-js no longer releases on its own. It lives in calimero-network/core under
`sdks/` and ships with core: one version (core's root `VERSION`), one tag,
one release.

- How core is released, and the rules every pull request title follows:
  [core's RELEASING.md](../../../RELEASING.md).
- How the SDKs are versioned, tested, generated and published:
  [sdks/AGENTS.md](../../../sdks/AGENTS.md).

Never edit the version in `package.json` by hand: `python3 scripts/sdks.py
version-sync` writes `VERSION` into it. `CHANGELOG.md` keeps the history
mero-js had as a separate repository (semantic-release, tags `mero-js-v*`);
new entries go in core's release notes.
