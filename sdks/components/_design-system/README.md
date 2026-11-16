# Calimero Design System

## Releasing with Changesets

This repository uses Changesets to version and publish workspace packages.

### Prerequisites
- pnpm installed (see `package.json`'s `packageManager` field)
- You are logged in to npm and allowed to publish `@calimero-network/*` packages
  - Run `npm whoami` to verify
  - If your npm account has 2FA, be ready to enter an OTP (or use an automation token)

### 1) Create a changeset
- Interactive (recommended):
```bash
pnpm changeset
```
  - Select packages to release
  - Choose bump type (patch/minor/major)
  - Enter a short summary

- Non-interactive (advanced): add a Markdown file under `.changeset/` with frontmatter like:
```
---
"@calimero-network/mero-ui": patch
"@calimero-network/mero-icons": patch
---

Your summary here.
```

### 2) Version packages and update changelogs
```bash
pnpm changeset version
```
This updates package versions and writes CHANGELOG entries. Commit the result if not auto-committed.

### 3) Build the workspace
```bash
pnpm -w build
```
Ensure all packages compile and produce artifacts in `dist/`.

### 4) Publish to npm
```bash
pnpm changeset publish
```
- If prompted for an OTP, enter the code from your authenticator
- For CI or non-interactive publishing, use an npm automation token with 2FA = automation

### 5) Push commits and tags
```bash
git push origin HEAD:main --follow-tags
```

### Notes
- Base branch is `main` (configured in `.changeset/config.json`)
- The Storybook app is named `@calimero-network/storybook-app` and is ignored by Changesets
- Node 20 LTS is pinned via `engines` and `.nvmrc` to avoid known pnpm/Node 22 registry issues
- If publish fails on OTP in CI, prefer using an npm automation token, or run the publish step locally
