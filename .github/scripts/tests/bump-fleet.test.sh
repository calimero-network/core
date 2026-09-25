#!/usr/bin/env bash
#
# Fixture tests for bump-fleet.sh. No runner, no token, no network.
#
#   bash .github/scripts/tests/bump-fleet.test.sh
#
# This exists because every defect this script has shipped was silent. The
# pnpm probe that died at exit 123 took out 7 of 12 consumers and read as
# "Process completed with exit code 123" with no hint of the cause; the
# absolute-path lockfile bug made three repositories sit out a release while
# the run looked fine. Both were reproducible in a directory of fixture files
# and neither was caught, because there was nowhere to put the fixture.
#
# The cases that matter are the ones where "nothing happened" and "this does not
# apply" have to stay distinguishable — exit 4 versus exit 3 — and the layouts
# where a plausible-looking rewrite silently edits the wrong file.

set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd -P)
BUMP="$HERE/../bump-fleet.sh"
[ -f "$BUMP" ] || { echo "cannot find bump-fleet.sh next to the tests"; exit 1; }

PASS=0
FAIL=0
ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

ok()   { PASS=$((PASS + 1)); printf '  ok    %s\n' "$1"; }
bad()  { FAIL=$((FAIL + 1)); printf '  FAIL  %s\n     %s\n' "$1" "$2"; }

# Each fixture is a real git checkout: --dry-run reverts by checking out, and
# refuses to run at all against a dirty tree.
mkfixture() {
  local d="$ROOT/$1"; mkdir -p "$d"; printf '%s\n' "$d"
}
commit() {
  ( cd "$1" && git init -q . && git add -A \
      && git -c user.email=t@t -c user.name=t commit -qm fixture )
}

expect_exit() {
  local want="$1" label="$2"; shift 2
  local out; out=$("$@" 2>&1); local got=$?
  if [ "$got" -eq "$want" ]; then ok "$label (exit $got)"
  else bad "$label" "expected exit $want, got $got: $(printf '%s' "$out" | tr '\n' ' ' | cut -c1-160)"; fi
}

# `-- "$pat"`, not `"$pat"`. A pattern that starts with a dash — asserting on
# `--tag 0.11.0-rc.2`, say — is otherwise read by grep as an option, and the
# assertion fails with "unrecognized option" for a rewrite that worked fine.
expect_file() {
  local f="$1" pat="$2" label="$3"
  if grep -qF -- "$pat" "$f" 2>/dev/null; then ok "$label"
  else bad "$label" "'$pat' not found in ${f##*/}"; fi
}

expect_absent() {
  local f="$1" pat="$2" label="$3"
  if grep -qF -- "$pat" "$f" 2>/dev/null; then bad "$label" "'$pat' is still present in ${f##*/}"
  else ok "$label"; fi
}

# ─────────────────────────────────────────────────────────────────────────────
echo "standalone app repository (logic/Cargo.toml)"
# ─────────────────────────────────────────────────────────────────────────────
D=$(mkfixture standalone); mkdir -p "$D/logic"
cat > "$D/logic/Cargo.toml" <<'EOF'
[dependencies]
calimero-sdk = { git = "https://github.com/calimero-network/core", tag = "0.11.0-rc.1" }
calimero-storage = { git = "https://github.com/calimero-network/core.git", tag = "0.11.0-rc.1" }

[package.metadata.calimero]
min-runtime-version = "0.11.0-rc.1"
EOF
commit "$D"
expect_exit 0 "rewrites the pins" bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock
expect_file "$D/logic/Cargo.toml" 'tag = "0.11.0-rc.2"' "  tag moved"
expect_file "$D/logic/Cargo.toml" 'min-runtime-version = "0.11.0-rc.2"' "  floor moved with it"
expect_absent "$D/logic/Cargo.toml" '0.11.0-rc.1' "  no stale tag left"
expect_exit 4 "second run is a no-op" bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock

# ─────────────────────────────────────────────────────────────────────────────
echo "workspace monorepo (apps/*/logic, one shared pin)"
# ─────────────────────────────────────────────────────────────────────────────
D=$(mkfixture workspace)
mkdir -p "$D/apps/one/logic/workflows/probes" "$D/apps/one/logic/workflows-parked" "$D/apps/two/logic" \
         "$D/.github/actions/install-cargo-mero"
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
members = ["apps/*/logic"]

[workspace.dependencies]
calimero-sdk            = { git = "https://github.com/calimero-network/core.git", tag = "0.11.0-rc.1" }
calimero-storage        = { git = "https://github.com/calimero-network/core.git", tag = "0.11.0-rc.1" }

[workspace.metadata.mero-apps]
min-runtime-version = "0.11.0-rc.1"
merod-image = "ghcr.io/calimero-network/merod:0.11.0-rc.1"
EOF
for a in one two; do
cat > "$D/apps/$a/logic/Cargo.toml" <<'EOF'
[package.metadata.calimero]
package = "com.calimero.x"
min-runtime-version = "0.11.0-rc.1"
EOF
done
echo 'image: ghcr.io/calimero-network/merod:0.11.0-rc.1' > "$D/apps/one/logic/workflows/e2e.yml"
echo 'image: ghcr.io/calimero-network/merod:0.11.0-rc.1' > "$D/apps/one/logic/workflows/probes/smoke.yml"
echo 'image: ghcr.io/calimero-network/merod:0.11.0-rc.1' > "$D/apps/one/logic/workflows-parked/blocked.yml"
printf 'inputs:\n  version:\n    default: "0.11.0-rc.1"\n' > "$D/.github/actions/install-cargo-mero/action.yml"
commit "$D"

expect_exit 0 "rewrites the workspace" bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock
expect_file "$D/Cargo.toml" 'tag = "0.11.0-rc.2"' "  workspace tag moved"
expect_file "$D/Cargo.toml" 'merod-image = "ghcr.io/calimero-network/merod:0.11.0-rc.2"' "  merod-image moved"
expect_file "$D/apps/one/logic/Cargo.toml" 'min-runtime-version = "0.11.0-rc.2"' "  app one's floor moved"
expect_file "$D/apps/two/logic/Cargo.toml" 'min-runtime-version = "0.11.0-rc.2"' "  app two's floor moved"
expect_file "$D/apps/one/logic/workflows/e2e.yml" 'merod:0.11.0-rc.2' "  scenario image moved"
# The repo's own checker globs one level and never sees probes/; drift there is
# invisible until a probe runs against a node two releases old.
expect_file "$D/apps/one/logic/workflows/probes/smoke.yml" 'merod:0.11.0-rc.2' "  probes/ scenario moved too"
# Parked scenarios are kept but not run by CI (mero-chat parks three). One left
# on an old image fails for THAT reason the day it is un-parked.
expect_file "$D/apps/one/logic/workflows-parked/blocked.yml" 'merod:0.11.0-rc.2' "  workflows-parked/ scenario moved too"
expect_file "$D/.github/actions/install-cargo-mero/action.yml" '"0.11.0-rc.2"' "  cargo-mero default moved"
expect_exit 4 "second run is a no-op" bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock

# An app whose floor trailed the tag is HEALED, not carried forward. This is the
# drift that started the rc.25 sweep: six repositories pinned one release with
# the floor still on the one before, each edited by hand by someone who moved
# the tag and forgot the floor.
printf '[package.metadata.calimero]\nmin-runtime-version = "0.9.9"\n' \
  > "$D/apps/two/logic/Cargo.toml"
expect_exit 0 "a drifted app is healed" \
  bash "$BUMP" --surface cargo --version 0.11.0-rc.3 --dir "$D" --no-lock
expect_file "$D/apps/two/logic/Cargo.toml" 'min-runtime-version = "0.11.0-rc.3"' "  drift healed"

# An app with no floor at all is a hard stop: check-app-metadata.sh rejects it,
# so opening the pull request would only spend a CI run to say so.
printf '[package.metadata.calimero]\npackage = "com.calimero.x"\n' \
  > "$D/apps/two/logic/Cargo.toml"
expect_exit 1 "an app with no floor stops the run" \
  bash "$BUMP" --surface cargo --version 0.11.0-rc.4 --dir "$D" --no-lock

# ─────────────────────────────────────────────────────────────────────────────
echo "monorepo toolchain pins (the ones no CI check covers)"
# ─────────────────────────────────────────────────────────────────────────────
#
# Every case below is a real file shape from calimero-network/apps, and every
# one of them was either drifting or a rewrite waiting to go wrong.
D=$(mkfixture toolchain)
mkdir -p "$D/apps/one/logic/workflows" "$D/apps/one/scripts" \
         "$D/apps/two/logic" "$D/apps/two/test/perf" \
         "$D/.github/actions/install-cargo-mero"
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
members = ["apps/*/logic"]

[workspace.dependencies]
calimero-sdk = { git = "https://github.com/calimero-network/core.git", tag = "0.11.0-rc.1" }

[workspace.metadata.mero-apps]
min-runtime-version = "0.11.0-rc.1"
merod-image = "ghcr.io/calimero-network/merod:0.11.0-rc.1"
EOF
for a in one two; do
  printf '[package.metadata.calimero]\npackage = "com.calimero.%s"\nmin-runtime-version = "0.11.0-rc.1"\n' \
    "$a" > "$D/apps/$a/logic/Cargo.toml"
done
echo 'image: ghcr.io/calimero-network/merod:0.11.0-rc.1' > "$D/apps/one/logic/workflows/e2e.yml"

# The action declares a DECOY input first. "Rewrite the first default: in the
# file" was correct only by luck — install-cargo-mero happens to declare one
# input — and would silently move the wrong key the day a second was added.
cat > "$D/.github/actions/install-cargo-mero/action.yml" <<'EOF'
inputs:
  cache-key-suffix:
    description: decoy
    required: false
    default: "none"
  version:
    description: The core tag to install cargo-mero from.
    required: false
    default: "0.11.0-rc.1"
EOF

# A plain installer script: the tag is a variable default, and the file quotes
# `cargo install --list` OUTPUT in a comment. The output line pairs a tag with
# the commit it resolved to, so rewriting it would invent a tag/sha pair that
# never existed.
cat > "$D/apps/one/scripts/ensure-cargo-mero.sh" <<'EOF'
#!/usr/bin/env bash
CARGO_MERO_TAG="${CARGO_MERO_TAG:-0.11.0-rc.1}"
# Installed from https://github.com/calimero-network/core
#   cargo install --git https://github.com/calimero-network/core --tag 0.11.0-rc.1 cargo-mero
# cargo install --list prints:
#   cargo-mero v0.1.0 (https://github.com/calimero-network/core?tag=0.11.0-rc.1#c2e8ec3f):
EOF

# A Makefile naming the image twice, once WITHOUT the registry prefix. Both
# lines have to move or the echo describes an image the pull does not fetch.
cat > "$D/apps/one/Makefile" <<'EOF'
node:
	@echo "==> docker pull merod:0.11.0-rc.1"
	docker pull ghcr.io/calimero-network/merod:0.11.0-rc.1
EOF

# A bundle manifest assembled by hand rather than by cargo-mero, so its floor
# is not one check-app-metadata.sh reads.
cat > "$D/apps/one/logic/build-bundle.sh" <<'EOF'
#!/usr/bin/env bash
# built against https://github.com/calimero-network/core
cat > manifest.json <<JSON
{
  "minRuntimeVersion": "0.11.0-rc.1"
}
JSON
EOF

# An unrelated script with a VERSION= that is NOT a core pin. Nothing here may
# touch it; a bump that corrupts a neighbouring version is worse than a bump
# that misses one.
printf '#!/usr/bin/env bash\nVERSION="2.4.1"   # our own tool\n' \
  > "$D/apps/one/scripts/package-icons.sh"

# A checksum-pinned installer. Moving the version alone makes the download fail
# its own integrity check — a harder break than being a release stale.
cat > "$D/apps/two/scripts-setup-cargo-mero.sh" <<'EOF'
#!/usr/bin/env bash
# from https://github.com/calimero-network/core
RELEASE=0.11.0-rc.1
CHECKSUM_aarch64_apple_darwin=9c28ec40692669cbf2249c07afa824ab
EOF

# A recorded measurement: the scenario pins an old node and the document next
# to it reports numbers measured against that node.
echo 'image: ghcr.io/calimero-network/merod:0.10.0' > "$D/apps/two/test/perf/perf.yml"
printf '| merod image | `ghcr.io/calimero-network/merod:0.10.0` |\n' > "$D/apps/two/test/perf/PERF.md"
# Prose.
printf 'The image used by workflows is `ghcr.io/calimero-network/merod:0.9.0`.\n' > "$D/apps/two/README.md"
commit "$D"

UNCLAIMED="$ROOT/unclaimed.txt"
expect_exit 0 "rewrites the toolchain too" \
  env UNCLAIMED_OUT="$UNCLAIMED" bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock

A="$D/.github/actions/install-cargo-mero/action.yml"
expect_file "$A" 'default: "0.11.0-rc.2"' "  version input default moved"
expect_file "$A" 'default: "none"' "  the decoy input above it was NOT touched"

S="$D/apps/one/scripts/ensure-cargo-mero.sh"
expect_file "$S" 'CARGO_MERO_TAG:-0.11.0-rc.2' "  installer tag default moved"
expect_file "$S" '--tag 0.11.0-rc.2 cargo-mero' "  the cargo install line moved"
expect_file "$S" '?tag=0.11.0-rc.1#c2e8ec3f' "  quoted --list output left alone (tag/sha pair)"

M="$D/apps/one/Makefile"
expect_file "$M" 'docker pull merod:0.11.0-rc.2' "  short-form image in the echo moved"
expect_file "$M" 'merod:0.11.0-rc.2"' "  ...and agrees with the pull below it"
expect_absent "$M" '0.11.0-rc.1' "  no stale image left in the Makefile"

expect_file "$D/apps/one/logic/build-bundle.sh" '"minRuntimeVersion": "0.11.0-rc.2"' \
  "  hand-written manifest floor moved"
expect_file "$D/apps/one/scripts/package-icons.sh" 'VERSION="2.4.1"' \
  "  an unrelated VERSION= is not corrupted"

# The three groups the report exists to keep visible.
expect_file "$D/apps/two/scripts-setup-cargo-mero.sh" 'RELEASE=0.11.0-rc.1' \
  "  a checksum-pinned installer is NOT rewritten"
expect_file "$UNCLAIMED" 'scripts-setup-cargo-mero.sh' "  ...and is named in the report"
expect_file "$UNCLAIMED" 'shasum -a 256' "  ...with the command that refreshes it"
expect_file "$D/apps/two/test/perf/perf.yml" 'merod:0.10.0' \
  "  a recorded measurement is NOT rewritten"
expect_file "$UNCLAIMED" 'test/perf/perf.yml' "  ...and is named in the report"
expect_file "$D/apps/two/README.md" 'merod:0.9.0' "  prose is NOT rewritten"
expect_file "$UNCLAIMED" 'README.md' "  ...and is named in the report"
expect_absent "$UNCLAIMED" 'UNEXPLAINED' "  nothing is left unexplained"

# A pin in a place none of the reasons cover has to be LOUD, not absorbed. This
# is the guard on the report staying meaningful as the repo grows.
printf 'image: ghcr.io/calimero-network/merod:0.9.9\n' > "$D/apps/one/logic/stray.yml"
( cd "$D" && git add -A && git -c user.email=t@t -c user.name=t commit -qm stray )
expect_exit 0 "an unrecognised pin still lets the bump through" \
  env UNCLAIMED_OUT="$UNCLAIMED" bash "$BUMP" --surface cargo --version 0.11.0-rc.3 --dir "$D" --no-lock
expect_file "$UNCLAIMED" 'UNEXPLAINED' "  ...but is reported as unexplained"
expect_file "$UNCLAIMED" 'stray.yml' "  ...naming the file"

# ─────────────────────────────────────────────────────────────────────────────
echo "pnpm catalog (versions live in pnpm-workspace.yaml)"
# ─────────────────────────────────────────────────────────────────────────────
D=$(mkfixture catalog); mkdir -p "$D/apps/one/app"
cat > "$D/pnpm-workspace.yaml" <<'EOF'
packages:
  - "apps/*/app"

catalog:
  "@calimero-network/mero-js": ^13.2.5
  "@calimero-network/mero-ui": ^1.5.1
EOF
cat > "$D/apps/one/app/package.json" <<'EOF'
{
  "name": "one",
  "dependencies": {
    "@calimero-network/mero-js": "catalog:",
    "@calimero-network/mero-icons": "0.0.6"
  }
}
EOF
touch "$D/pnpm-lock.yaml"
commit "$D"

expect_exit 0 "bumps through the catalog" \
  bash "$BUMP" --surface npm --pkg @calimero-network/mero-js=13.2.9 --dir "$D" --no-lock
expect_file "$D/pnpm-workspace.yaml" '"@calimero-network/mero-js": ^13.2.9' "  catalog entry moved, ^ kept"
expect_file "$D/apps/one/app/package.json" '"@calimero-network/mero-js": "catalog:"' "  package.json left alone"
expect_exit 4 "second run is a no-op" \
  bash "$BUMP" --surface npm --pkg @calimero-network/mero-js=13.2.9 --dir "$D" --no-lock

# The regression this guards: "catalog:" has no digits, so the numeric guard
# skipped it and the run exited 4 — a repository silently receiving no bumps
# while reporting success.
expect_exit 0 "a literal version alongside the catalog still moves" \
  bash "$BUMP" --surface npm --pkg @calimero-network/mero-icons=0.0.7 --dir "$D" --no-lock
expect_file "$D/apps/one/app/package.json" '"@calimero-network/mero-icons": "0.0.7"' "  literal dep moved"

expect_exit 4 "a major is skipped by default" \
  bash "$BUMP" --surface npm --pkg @calimero-network/mero-js=15.0.0 --dir "$D" --no-lock
expect_exit 0 "--allow-major crosses it" \
  bash "$BUMP" --surface npm --pkg @calimero-network/mero-js=15.0.0 --dir "$D" --no-lock --allow-major
expect_exit 3 "a package nobody declares is 'not applicable'" \
  bash "$BUMP" --surface npm --pkg @calimero-network/nothing=1.0.0 --dir "$D" --no-lock

# ─────────────────────────────────────────────────────────────────────────────
echo "nested workspace roots (calimero-studio: neither known path)"
# ─────────────────────────────────────────────────────────────────────────────
# Two workspace roots deep in the tree, each with its own lockfile, plus the
# merod images that have to move with them. Before this shape existed the
# script exited 3 here — "this does not apply" — and calimero-studio's pin sat
# twelve releases behind because nothing ever opened a pull request.
D=$(mkfixture nested)
mkdir -p "$D/foundation-app/base/logic/crates/service" \
         "$D/foundation-app/base/test" \
         "$D/foundation-app/recipes/tally" \
         "$D/foundation-app/overlays/rooms/test"
cat > "$D/foundation-app/base/logic/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/service"]

[workspace.dependencies]
calimero-sdk = { git = "https://github.com/calimero-network/core", tag = "0.11.0-rc.1" }
calimero-storage = { git = "https://github.com/calimero-network/core", tag = "0.11.0-rc.1" }
EOF
echo '[[package]]' > "$D/foundation-app/base/logic/Cargo.lock"
# A MEMBER crate: inherits the pin, carries no git URL. Must not be rewritten,
# and must not be counted as a root to resolve.
cat > "$D/foundation-app/base/logic/crates/service/Cargo.toml" <<'EOF'
[package]
name = "svc"

[dependencies]
calimero-sdk = { workspace = true }
EOF
cat > "$D/foundation-app/recipes/Cargo.toml" <<'EOF'
[workspace]
members = ["tally"]

[workspace.dependencies]
calimero-sdk = { git = "https://github.com/calimero-network/core", tag = "0.11.0-rc.1" }
EOF
echo '[[package]]' > "$D/foundation-app/recipes/Cargo.lock"
cat > "$D/foundation-app/base/test/smoke.workflow.yml" <<'EOF'
nodes:
  image: ghcr.io/calimero-network/merod:0.11.0-rc.1
EOF
cat > "$D/foundation-app/overlays/rooms/test/smoke.workflow.yml" <<'EOF'
nodes:
  image: ghcr.io/calimero-network/merod:0.11.0-rc.1
EOF
commit "$D"

CH="$ROOT/nested-changed.txt"
expect_exit 0 "bumps every nested root" env CHANGED_FILES_OUT="$CH" \
  bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock
expect_file "$D/foundation-app/base/logic/Cargo.toml" 'tag = "0.11.0-rc.2"' "  base logic root moved"
expect_file "$D/foundation-app/recipes/Cargo.toml" 'tag = "0.11.0-rc.2"' "  recipes root moved"
expect_file "$D/foundation-app/base/test/smoke.workflow.yml" 'merod:0.11.0-rc.2' "  base merod image moved"
expect_file "$D/foundation-app/overlays/rooms/test/smoke.workflow.yml" 'merod:0.11.0-rc.2' "  overlay merod image moved"
expect_absent "$D/foundation-app/base/logic/crates/service/Cargo.toml" '0.11.0-rc.2' "  a member crate is left alone"
expect_file "$CH" 'foundation-app/recipes/Cargo.toml' "  changed-files names the nested manifest"
expect_absent "$CH" 'crates/service/Cargo.toml' "  ...and not the member crate"

expect_exit 4 "second run is a no-op" \
  bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock

# One root behind the other IS the drift, so it must not read as "already done".
# Reporting off the first manifest alone would have hidden exactly this.
( cd "$D" && perl -i -pe 's{0\.11\.0-rc\.2}{0.11.0-rc.1}g' foundation-app/recipes/Cargo.toml \
    && git -c user.email=t@t -c user.name=t commit -qam skew )
expect_exit 0 "one root left behind is not 'already pinned'" \
  bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock
expect_file "$D/foundation-app/recipes/Cargo.toml" 'tag = "0.11.0-rc.2"' "  the straggler moved"

# A tree whose only core pin is at a nested path the script does not know still
# has to be distinguishable from a tree with no pin at all.
D=$(mkfixture nested-only-member); mkdir -p "$D/somewhere/deep"
cat > "$D/somewhere/deep/Cargo.toml" <<'EOF'
[dependencies]
calimero-sdk = { git = "https://github.com/calimero-network/core", tag = "0.11.0-rc.1" }
EOF
commit "$D"
expect_exit 0 "an unconventional path is still bumped" \
  bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock
expect_file "$D/somewhere/deep/Cargo.toml" 'tag = "0.11.0-rc.2"' "  ...and rewritten"

# ─────────────────────────────────────────────────────────────────────────────
echo "no surface at all"
# ─────────────────────────────────────────────────────────────────────────────
D=$(mkfixture bare); echo '{}' > "$D/package.json"; commit "$D"
expect_exit 3 "no contract anywhere" bash "$BUMP" --surface cargo --version 0.11.0-rc.2 --dir "$D" --no-lock

# ─────────────────────────────────────────────────────────────────────────────
echo "tee — the fleet image bundles a merod"
# ─────────────────────────────────────────────────────────────────────────────
#
# Two versions in ONE object, which is what makes this different from `tauri`:
# there the document's own `version` is the first key and position is enough, so
# a positional rewrite here would move whichever version happened to come first.
# These cases pin that both fields move, and that the OTHERS do not.
mkversions() {
  mkdir -p "$1/mero-tee"
  cat > "$1/mero-tee/versions.json" <<EOF
{
  "traefikVersion": "3.5.0",
  "nodeExporterVersion": "1.9.1",
  "vectorVersion": "0.50.0",
  "imageVersion": "$2",
  "merodVersion": "$3"
}
EOF
}

# The full shape mero-tee actually has: the image version is ONE version in
# THREE files, and the docs state both pins. A surface that moved only the JSON
# produced a pull request that failed the consumer's own guards — which is what
# the first generated one would have done.
mkkms() {
  mkdir -p "$1/mero-kms"
  cat > "$1/mero-kms/Cargo.toml" <<EOF
[package]
name = "mero-kms-phala"
version = "$2"

[dependencies]
serde = { version = "1.0.200" }
EOF
  cat > "$1/Cargo.lock" <<EOF
[[package]]
name = "serde"
version = "1.0.200"

[[package]]
name = "mero-kms-phala"
version = "$2"
EOF
}
mkteedocs() {
  mkdir -p "$1/docs/src/content/docs/operate" "$1/docs/dist"
  cat > "$1/docs/src/content/docs/operate/config-reference.mdx" <<EOF
| \`imageVersion\` | the image, currently \`$2\` |
| \`merodVersion\` | the core tag, currently \`$3\` |

The interim 25.10 release reached EOL and was dropped.
EOF
  echo "imageVersion 9.9.9" > "$1/docs/dist/index.html"
}

D=$(mkfixture tee-companions); mkversions "$D" 2.3.56 0.11.0-rc.35
mkkms "$D" 2.3.56; mkteedocs "$D" 2.3.56 0.11.0-rc.35; commit "$D"
expect_exit 0 "the companion files move with the image version" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"
expect_file "$D/mero-kms/Cargo.toml" 'version = "2.3.57"' "  the KMS package version tracks imageVersion"
expect_file "$D/mero-kms/Cargo.toml" 'serde = { version = "1.0.200" }' "  a dependency version is NOT rewritten"
expect_file "$D/Cargo.lock" 'name = "mero-kms-phala"' "  the lock still names the package"
expect_absent "$D/Cargo.lock" 'version = "2.3.56"' "  the lock entry moved"
expect_file "$D/Cargo.lock" 'version = "1.0.200"' "  an unrelated lock entry did not"
expect_file "$D/docs/src/content/docs/operate/config-reference.mdx" '`2.3.57`' "  the docs state the new image"
expect_file "$D/docs/src/content/docs/operate/config-reference.mdx" '`0.11.0-rc.39`' "  ...and the new core tag"
expect_file "$D/docs/src/content/docs/operate/config-reference.mdx" 'interim 25.10 release' "  prose about another version is untouched"
expect_file "$D/docs/dist/index.html" 'imageVersion 9.9.9' "  built output under dist/ is left alone"

# Docs drift on their own, and a sweep that matched the OLD value would do
# nothing in exactly that case — which is the state mero-tee was really in, two
# releases apart. Anchoring on the key makes it self-healing.
D=$(mkfixture tee-docs-drifted); mkversions "$D" 2.3.56 0.11.0-rc.35
mkkms "$D" 2.3.56; mkteedocs "$D" 2.3.52 0.11.0-rc.33; commit "$D"
expect_exit 0 "already-drifted docs still get corrected" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"
expect_file "$D/docs/src/content/docs/operate/config-reference.mdx" '`0.11.0-rc.39`' "  a doc two releases behind is repaired"
expect_absent "$D/docs/src/content/docs/operate/config-reference.mdx" '2.3.52' "  ...and its stale image version is gone"

# A repository with no KMS crate and no docs is not an error: the surface is
# shaped for mero-tee but must not hard-fail on a consumer without those.
D=$(mkfixture tee-json-only); mkversions "$D" 2.3.56 0.11.0-rc.35; commit "$D"
expect_exit 0 "versions.json alone still bumps" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"
expect_file "$D/mero-tee/versions.json" '"merodVersion": "0.11.0-rc.39"' "  the pin moved anyway"

D=$(mkfixture tee-image); mkversions "$D" 2.3.56 0.11.0-rc.35; commit "$D"
expect_exit 0 "the bundled merod moves" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"
expect_file "$D/mero-tee/versions.json" '"merodVersion": "0.11.0-rc.39"' "  merodVersion took the release"
expect_file "$D/mero-tee/versions.json" '"imageVersion": "2.3.57"' "  imageVersion patch-incremented"
expect_file "$D/mero-tee/versions.json" '"traefikVersion": "3.5.0"' "  an unrelated version is untouched"
expect_file "$D/mero-tee/versions.json" '"vectorVersion": "0.50.0"' "  ...and so is the one after it"

# Re-running a release must be a no-op, not a second image version. Without the
# early exit this would walk imageVersion forward on every re-run.
D=$(mkfixture tee-already); mkversions "$D" 2.3.57 0.11.0-rc.39; commit "$D"
expect_exit 4 "already bundling this merod" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"
expect_file "$D/mero-tee/versions.json" '"imageVersion": "2.3.57"' "  the image version did NOT drift on a re-run"

# `nothing to do here` and `this does not apply here` stay distinguishable.
D=$(mkfixture tee-absent); echo '{}' > "$D/package.json"; commit "$D"
expect_exit 3 "no versions.json is not-applicable, not failure" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"

# A version with no patch field would silently print `2.4.0` from awk's empty $3
# if the increment were not explicit about it.
D=$(mkfixture tee-two-field); mkversions "$D" 2.4 0.11.0-rc.35; commit "$D"
expect_exit 0 "a two-field image version still bumps" \
  bash "$BUMP" --surface tee --version 0.11.0-rc.39 --dir "$D"
expect_file "$D/mero-tee/versions.json" '"imageVersion": "2.4.1"' "  missing patch reads as 0 and becomes 1"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
