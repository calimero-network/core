# Changelog entries

Every pull request that changes behaviour adds **its own file** here instead of
editing `CHANGELOG.md`. No two pull requests touch the same lines, so none
conflicts with another over the changelog. The release pull request
(`chore(release): cut X`) gathers the files into `CHANGELOG.md`'s dated section
and deletes them:

```bash
python3 scripts/changelog.py check            # every file here is well formed (CI runs it)
python3 scripts/changelog.py preview          # what the next release's section will say
python3 scripts/changelog.py release 1.0.0    # the release PR: gather into CHANGELOG.md, delete
```

## A file

- **Name**: a short slug of the change, `<slug>.md`
  (`snapshot-build-reads-its-generation-under-the-lock.md`). The name never
  reaches `CHANGELOG.md`; the release lists entries in the order they merged.
- **Body**: one or more sections, each a `### <Section>` heading and its
  bullets, written as they will read in the release:
  `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`,
  `Dependencies`, `Documentation` or `CI`.

```markdown
### Fixed

- **A snapshot built during an install holds the right state** (`calimero-log`):
  what was wrong, what changed, and what an operator must do, if anything.
```

- **A hard cutover** says **Hard cutover** in its entry, with what operators
  must do (the `Compatibility door` check looks for it in the PR's file here).
- **A deprecation** goes under `### Deprecated` and names its replacement.
