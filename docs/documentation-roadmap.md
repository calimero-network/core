# Calimero Documentation Roadmap — "Next Level"

Status: **proposal / for discussion** · Owner: docs · Last updated: 2026-06-26

This is the plan to take Calimero's documentation from "good content, dated and
inconsistent presentation" to a best-in-class developer documentation product.
It is grounded in a full audit of the existing docs, a design/UX audit, a
coverage-gap analysis against the whole core, and a benchmark of how Stripe,
libp2p/IPFS, Tailscale, Linear, Matrix and the Diátaxis framework set the bar.

---

## 1. Where we are (honest assessment)

We are not starting from zero. The audit found **~59 documents, roughly two-thirds
genuinely strong on content** — the crate deep-dives, `membership-and-leave`,
the new protocol reference, the host-functions and storage references. The
problem is **not the content; it is everything around it.**

Three structural problems drag the whole thing down:

1. **No information architecture.** 47+ pages accreted by audience-blind growth
   and a per-PR auto-update bot. There is no audience routing, no Diátaxis
   separation (tutorials vs how-to vs reference vs explanation are mixed on the
   same pages), and overlapping pages that contradict each other
   (`tee-mode` vs `tee-fleet-ha` vs `auto-follow` vs `protocol/governance`;
   `local-governance` vs `protocol/governance`; `dag.html` superseded by
   `protocol/operations`).

2. **The presentation looks dated ("ugly").** The design audit is blunt: failing
   contrast (lime `#a5ff11` and `--text-dim` fail WCAG AA in several places),
   body text too small (14px), monospace overused in UI chrome, card-on-card
   visual fatigue with no whitespace to rest the eye, no real type scale, an
   inconsistent diagram palette (hand-SVG vs the new engine vs d3, 6+ colour
   sets), a half-baked light theme, no code-copy buttons, no syntax
   highlighting, tables that overflow on mobile, and a search that builds its
   index on first keystroke.

3. **The authoring & freshness model is fragile.** Content is hand-written HTML
   (high friction, no PR-friendly diffs), and an external bot regenerates pages
   per-PR with no editorial pass — which is exactly why nav labels were literal
   PR titles. There is no signal of what is *stable* vs *draft* vs *blueprint*.

Coverage gaps compound it: **no SDK API reference, no CLI reference
(merod/meroctl have ~60 routes), no admin-API reference (~60 endpoints), no
install/quickstart for operators, no operational runbooks**, and example apps
documented conceptually with no annotated code.

The goal of this roadmap is to fix all three at once: **re-architect, re-skin,
and re-tool — without throwing away the strong content or the bespoke animated
diagrams that make us distinctive.**

---

## 2. Vision

> One documentation product, four clearly-routed audiences, every page in a
> single Diátaxis mode, on a fast modern platform, with one consistent visual
> language and a small set of unforgettable interactive diagrams.

Concretely, a reader should:

- land on a page that asks **"who are you / what do you want to do"** and routes
  them (Tailscale/Vercel pattern), not a table of contents;
- never confuse a tutorial with a reference — each page declares its mode;
- read the **protocol spec** as a normative, versioned document distinct from the
  friendly **concepts** explainers (libp2p/Matrix pattern);
- find anything via an instant **⌘K search**;
- see **one diagram language** throughout, with a handful of step-controllable
  animated showpieces;
- always know whether what they're reading is **Stable / Draft / Planned** and
  **which version** it pins to.

---

## 3. Target information architecture

Split by **audience track first**, then apply Diátaxis modes within each track.
This kills the "everyone fights over Getting Started" failure.

| Track | Reader | Diátaxis emphasis | Sections |
|---|---|---|---|
| **Build** | App developers writing WASM apps | Tutorials + How-to + Reference | Quickstart → How-to guides → **SDK & API reference** → examples |
| **Operate** | Node operators & deployers | How-to + Reference | Install/run → **config reference** → **CLI reference** → networking/NAT → observability & troubleshooting/runbooks |
| **Protocol** | Reimplementers, auditors | Explanation + **normative Reference** | **Concepts** (explainers) → **Spec** (normative, versioned) → message/state/storage reference |
| **Contribute** | Core developers | Explanation + How-to | Architecture explainers → crate map → dev setup → testing → RFC/release process |

Rules we enforce in review:

- **Concepts ≠ Spec.** A friendly Concepts explainer for CRDTs, causal auth,
  gossip/sync; a separate normative Spec an independent implementer can build
  from. Concepts link *into* the spec, never duplicate it. (The current
  `protocol/` reference becomes the spine of the Protocol track; the explanatory
  `concepts.html`/`system-overview.html` become Concepts.)
- **Each page declares one mode** (a badge): Tutorial / How-to / Reference /
  Explanation, plus a status (Stable / Draft / Planned) and, for the spec, a
  version.
- **One canonical "Start here" per track**, surfaced by the routed landing page.

### Page disposition (keep / refactor / write / retire)

- **Keep & re-skin:** crate deep-dives, `membership-and-leave`, `system-overview`,
  `concepts`, `migrations`, `glossary`, host-functions & storage references, the
  new `protocol/*` chapters.
- **Refactor / merge:** consolidate the four TEE pages into one "TEE & Fleet"
  reference; fold `local-governance` implementation detail into the Contribute
  track and point the protocol story at `protocol/governance`; refresh stale
  `app-lifecycle` (multi-service bundles) and `dependency-explorer`.
- **Write (high-value gaps):** SDK API reference; collections reference; admin-API
  reference (from the ~60 routes); merod/meroctl CLI reference; operator install
  + quickstart + runbooks; app-upgrade guide; xcall & blob guides; contributor
  getting-started.
- **Retire / absorb:** `dag.html` (superseded), thin `tee-mode.html`, duplicate
  entry points; standardise the 12 app READMEs against one template.

---

## 4. The pivotal decision — platform & tooling

This forks the entire execution plan, so it is called out first.

The benchmark is unambiguous that our real liabilities are *authoring in HTML*
(no Markdown diffs, high friction) and *maintaining bespoke nav/search/versioning*.
Two viable paths:

### Option A — Migrate content to **Astro Starlight**, keep our identity (recommended)
- Content becomes **Markdown/MDX** → PR-friendly diffs, contributors can write.
- **Built-in Pagefind search, dark mode, responsive nav, ⌘K, perfect-Lighthouse
  baseline** replace our bespoke JS — less code to own, faster site.
- **Astro islands let us mount the existing animated-SVG engine** as components on
  otherwise-static pages — we keep the diagrams, we don't rewrite them.
- We port our dark theme + accent into a Starlight custom theme so it still looks
  like *us*, just fixed (contrast, type scale, spacing).
- **Cost / gap:** versioning isn't native (community plugin or Git-branch
  strategy); a migration project to move ~50 pages of HTML → MDX.

### Option B — Stay bespoke, do a deep in-place redesign
- Keep `architecture/*.html` + `nav.js` + `seq-diagram.js`; invest only in a CSS
  design-system rewrite and IA reshuffle.
- **Pro:** no migration; we already control everything.
- **Con:** we keep paying the HTML-authoring and bespoke-search/nav/versioning
  tax forever; "next level" findability (⌘K, pre-built index) and Markdown
  authoring remain DIY.

**Recommendation: Option A (Starlight, SVG engine as islands)** unless
cross-version spec switching is a day-one hard requirement — in which case
Docusaurus (native versioning, heavier JS) is the alternative. This supersedes
the earlier "keep bespoke HTML" call now that the bar is "next level," but it is
the owner's decision; everything below is written to work under either option.

---

## 5. Design system overhaul (the "ugly" fixes)

Independent of platform, the visual language gets a single coherent spec.
Highest-leverage moves, in order:

1. **Reading & contrast.** Body text 16px / line-height ~1.7, measure capped at
   ~70ch. Lighten `--text-dim` (`#8b949e`→`~#a8b1ba`) and reserve the lime accent
   for active/focus/links only — everything else in a tight neutral gray scale.
   Audit every pair to WCAG AA.
2. **Type scale.** Adopt a real modular scale (~1.2×) and apply it consistently;
   lighter, larger headings (weight 300–500, drop the −1px tracking). Monospace
   only for code/commands, never for nav, breadcrumbs, or subtitles.
3. **Whitespace over borders.** Replace card-on-card chrome with section dividers
   + negative space; one dominant corner radius (~12–16px); barely-there
   elevation on hover instead of border-flashing.
4. **One diagram palette.** Extract diagram colours to CSS variables; the bespoke
   engine and any Mermaid theme share one palette, one arrow/lifeline vocabulary,
   one label type scale.
5. **Code presentation.** Syntax highlighting (Rust/JSON/TOML), copy-to-clipboard
   on every block, per-heading anchor links ("copy link"), horizontally
   scrollable tables on mobile.
6. **Callouts for the four voices.** A small fixed component set: `Note`
   (explanation), `Warning` (footgun), and a distinct **`Normative`/MUST** style so
   spec implementers can scan obligations (Matrix convention).
7. **Navigation feel.** ⌘K palette + pre-built search index; sticky sidebar with
   scroll-spy; real breadcrumbs; prev/next within a track; an on-page outline for
   long spec pages; respect `prefers-reduced-motion`.
8. **Complete the light theme** and ship both to AA.

Target "feel": Linear/Tailscale restraint — one accent as rare punctuation,
generous whitespace, authority through typography not ornament.

---

## 6. Diagram strategy — two tiers

Keep the bespoke engine as an asset; stop hand-drawing routine figures.

- **Tier 1 — Mermaid (diagrams-as-code) for ~90% of figures.** Sequence diagrams
  (handshakes, sync rounds), state machines (peer/protocol states), simple
  topology. Lives in the repo, diffs in PRs, never goes stale, any contributor
  edits it. (D2 is the upgrade path if we outgrow Mermaid's layout.)
- **Tier 2 — bespoke animated SVG for 5–10 hero showpieces:** the gossip/sync
  walkthrough, the CRDT merge convergence, the causal-auth / state-divergence
  flow, the life-of-an-operation. Step-controllable (play/pause/scrub), static
  fallback under reduced-motion.
- **One visual language across both tiers** (palette, arrows, labels).

This reconciles the earlier "all animated hand-SVG" preference with
maintainability: hero diagrams stay hand-built and animated; the long tail moves
to Mermaid so it can't rot.

---

## 7. Content plan by track (what to write)

**Build** — Quickstart (15-min app); How-tos (persist with a CRDT map; handle
peer events; emit & handle events; xcall another context; blobs); **SDK reference**
(all `#[app::*]` macros, the collection types `UnorderedMap`/`Vector`/`Authored*`/
`Frozen`/`Shared`/… with semantics, the `env` host functions); annotated example
walkthroughs replacing the thin example pages.

**Operate** — Install (binaries vs source vs Docker); Quickstart (init→run→join);
**config reference** (refresh, all sections); **CLI reference** (merod + meroctl,
every subcommand/flag with examples); **admin-API reference** (the ~60 endpoints,
ideally OpenAPI-generated); networking/NAT/relays/bootstrap; observability
(metrics + dashboards); **runbooks** (upgrade a node, rotate keys, recover from
divergence, back up/restore); troubleshooting.

**Protocol** — Concepts explainers (scopes, CRDTs, causal auth, gossip/sync);
the normative **Spec** = the current `protocol/*` chapters promoted to versioned,
normative status with MUST/SHOULD language and the byte-level appendices; fill
the spec gaps the audit found: **blob protocol, TEE attestation protocol, xcall
message format, migration/upgrade protocol, identity/key rotation, sync error &
divergence recovery, capability inheritance edge cases.**

**Contribute** — Contributor getting-started (build, test, debug with merodb);
crate-map narrative; actor-communication contracts; testing strategy (unit/
integration/e2e/sync-sim); ABI stability rules; RFC/ADR + release process.

---

## 8. Auto-update pipeline integration

The per-PR doc bot is useful for keeping *generated* crate/reference pages fresh,
but it must never touch *authored* narrative. Policy:

- **Authored zone** (tracks, concepts, spec, design system) — hand-written,
  fenced **out** of the bot (paths-ignore), reviewed like code.
- **Generated zone** (crate deep-dives, API/CLI/endpoint references) — keep
  auto-generating, but from structured sources (rustdoc, the CLI's own `--help`,
  the server's route table / OpenAPI) rather than free-form LLM HTML, and render
  into the design system.
- The bot's job becomes "regenerate the reference tables," not "write prose."
- **Immediate action regardless of platform:** fence `architecture/protocol/**`
  out of `doc-update.yaml` before more authored work lands on `master`.

---

## 9. Phased execution

**Phase 0 — Decide & de-risk (this week).** Pick the platform (§4). Fence the
authored zone from the bot. Lock the design tokens (type scale, colour, spacing)
and the diagram palette. Stand up a spike of the chosen platform with our theme +
one ported page + one mounted animated diagram.

**Phase 1 — Skeleton & shell.** Build the four-track IA, the routed landing page,
the page-mode/status/version badges, ⌘K search, and the design system. Migrate
(or re-skin) the strong existing pages into the shell. Outcome: the site *looks*
next-level and is correctly organised, even where content is still thin.

**Phase 2 — Close the blocking gaps.** SDK reference, CLI reference, admin-API
reference, operator install/quickstart. These unblock real users today.

**Phase 3 — Protocol spec hardening.** Promote `protocol/*` to a versioned
normative spec; add the missing protocol chapters (blobs, TEE attestation, xcall,
migration, rotation, divergence recovery); convert routine figures to Mermaid;
build the 5–10 hero animated diagrams.

**Phase 4 — Operate depth & polish.** Runbooks, troubleshooting, observability,
deployment; app-README standardisation; example walkthroughs; light-theme AA;
performance pass.

**Phase 5 — Sustain.** Generated-zone pipeline from rustdoc/CLI/OpenAPI; docs CI
(link-check, contrast-check, build); versioning workflow; a short docs style
guide so quality holds.

---

## 10. Success criteria

- A new app developer ships a working app from the Quickstart in **< 30 min**.
- An operator stands up and joins a node from docs alone, **no source reading**.
- An independent implementer can build a conformant node from the **Protocol spec**
  without reading Rust.
- Every page: one Diátaxis mode, a status badge, AA contrast, ⌘K-findable.
- Lighthouse ≥ 95 across the board; search is instant; both themes pass AA.
- One diagram language; no contradictory/duplicate pages; no stale-by-months pages.

---

## Appendix — source analyses

This roadmap synthesises four audits (doc inventory & critique; design/UX audit;
coverage-gap matrix vs the codebase; best-in-class benchmark). Key exemplars
referenced: Diátaxis (mode separation), Stripe (three-pane reference, single-page
deep links), libp2p/IPFS (Concepts vs Spec), Matrix/Noise (normative spec style),
Tailscale/Linear/Vercel (visual restraint), Astro Starlight (tooling).

---

## Execution status (as built in `docs-site/`)

- **Phase 0 — Platform** ✅ Astro Starlight with the Calimero dark theme; the
  bespoke animated-SVG diagram engine mounted as an island.
- **Phase 1 — Protocol track** ✅ All 10 core chapters + storage appendix ported
  to MDX; structural SVGs are `.astro` diagram components, sequences are islands.
- **Phase 2 — Build / Operate / Contribute** ✅ Four-track IA; SDK, CLI
  (merod/meroctl), config, and admin-API references grounded in code.
- **Phase 3 — Spec hardening** ✅ Added Blob Transport, TEE Attestation,
  Cross-Context Calls, Application Upgrades; RFC 2119 Normative callouts; schema
  version pins; conformance statement.
- **Phase 4 — Accuracy & depth** ✅ Verified the flagged cautions against source
  (fixed real flag/endpoint/macro errors); added observability, troubleshooting,
  runbooks, and example walkthroughs; completed the light theme.
- **Phase 5 — Sustain** ◑ Docs CI (`docs-ci.yml`: build + internal-link check on
  every docs PR), a zero-dependency link checker (`npm run check`), and a
  contributor guide (`/contribute/docs/`). Remaining below.

### Sustain decisions

- **Versioning — deferred to 1.0.** Starlight has no native versioning and the
  protocol is pre-1.0 (breaking freely). Pinning schema versions in the spec
  text is sufficient for now; adopt Git-branch snapshots or a plugin when the
  first stable wire version ships.
- **Generated references — incremental.** The CLI / admin-API / config tables are
  currently hand-curated and verified. The durable fix is to generate them from
  the sources of truth (clap `--help`, the axum route table / an OpenAPI export,
  and rustdoc) so they can't drift; this is the main outstanding sustain task.
- **Legacy cutover.** `docs-site.yml` (manual) publishes the Starlight site and
  preserves the old `architecture/` site under `/architecture-legacy/`. Cutover:
  (1) merge; (2) run the workflow to verify the live deploy; (3) retire
  `pages.yml` and switch `docs-site.yml` to deploy on push; (4) eventually delete
  `architecture/` once nothing links to it (and retarget the doc-update bot, or
  repoint it to regenerate the *generated* reference pages only).
