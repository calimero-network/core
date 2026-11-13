# Docs improvement backlog

A living checklist for taking the Calimero docs from "complete first draft" to
polished product. Grouped by theme; check items off as they land.

## A · Visual design & theme
- [x] 1. Add a Calimero logo in the header + a favicon.
- [ ] 2. Generate per-page social/OG images for link previews.
- [x] 3. 404 page — use Starlight's themed default (a custom content/`src/pages` 404 collides with Starlight's injected `/404` route and warns).
- [ ] 4. Lock a modular type scale and apply it consistently.
- [ ] 5. Replace remaining card-heavy patterns with dividers + whitespace.
- [ ] 6. Add a subtle, uncluttered hero treatment on the landing.
- [ ] 7. Define semantic color tokens (success/warning/error/info) and use them.
- [ ] 8. Audit every text/background pair to WCAG AA in both themes.
- [ ] 9. Give `Normative` callouts a distinct visual style vs generic caution.
- [x] 10. Add `theme-color` meta + polish the dark/light toggle affordance.

## B · Navigation & information architecture
- [x] 11. Add "Edit this page on GitHub" links.
- [x] 12. Show "Last updated" (from git) on each page.
- [ ] 13. Add prev/next pagination scoped within a track.
- [ ] 14. Tune the on-page table of contents (depth + scroll-spy) for long pages.
- [ ] 15. Surface the ⌘K search hint in the header.
- [ ] 16. Ensure breadcrumbs reflect track › page.
- [x] 17. Group the Protocol sidebar into Core / Subsystems / Appendix.
- [ ] 18. Add "See also / related pages" blocks at the foot of each page.
- [ ] 19. Make the role chooser on the landing more prominent than the cards.
- [x] 20. Add a Glossary page and link key terms to it.

## C · Diagrams & interactivity
- [ ] 21. Add play/pause/scrub controls to animated sequence diagrams.
- [x] 22. Unify all diagram palettes to shared CSS variables. (The "Diagrams" keys in `src/styles/theme.css`; contribute/docs "Diagrams".)
- [ ] 23. Make diagrams scale/scroll gracefully on mobile.
- [ ] 24. Add a reduced-motion static caption listing each diagram's steps.
- [x] 25. Apply the shared diagram-component pattern beyond the Protocol track. (`src/components/diagrams/`: Build, Ops, Contrib and Org diagrams.)
- [ ] 26. Add an interactive end-to-end "life of a call" walkthrough (leader run, log entry, followers applying the diff).
- [ ] 27. Add alt text / aria descriptions to every diagram.

## D · Code & spec presentation
- [x] 29. Copy-to-clipboard on all code blocks (Expressive Code default).
- [ ] 30. Add language tabs for multi-language examples.
- [ ] 31. Add filename/title headers to code blocks.
- [ ] 32. Add line highlighting/focus to long code samples.
- [ ] 33. Add per-heading anchor "copy link" affordance.
- [x] 34. Make all spec tables horizontally scrollable on mobile.
- [ ] 35. Add a per-chapter "Normative requirements" index.
- [ ] 36. Standardize struct vs pseudocode fences across chapters.

## E · Search & findability
- [x] 37. Built-in full-text search (Pagefind) shipped.
- [ ] 38. Group search results by track.
- [ ] 39. Add keyboard-shortcut hints and verify focus management.
- [x] 40. Add an `llms.txt` machine-readable index for AI tools.
- [x] 41. Add `robots.txt` (sitemap already generated).
- [ ] 42. Add canonical URLs + verify per-page descriptions for SEO.

## F · Build track content
- [ ] 43. Expand the quickstart into a full tutorial with expected output.
- [x] 44. Add a "first app from scratch" tutorial (scaffold → state → method → test).
- [x] 45. Document the `TestApp` testing harness with examples (`build/testing`).
- [x] 46. How-to: model data with the right collection or `Table` (`build/collections`).
- [x] 47. How-to: emit & handle events.
- [x] 48. How-to: cross-context calls (link `/protocol/xcall/`).
- [x] 49. How-to: blobs and sealed files (`build/guides/blobs`).
- [x] 50. Document access control (explicit checks against the caller, `PerUser`, `Frozen`) with examples (`build/guides/access-control`).
- [ ] 51. Generate an SDK API reference from rustdoc.
- [x] 52. How-to: ship v2 (migrations) with troubleshooting. (`build/ship-v2`.)

## G · Operate track content
- [x] 53. Node API reference with full request/response schemas (OpenAPI); routes and credentials are listed (`operate/admin-api`).
- [ ] 54. Generate the merod/meroctl CLI reference from `--help` (hand-written from the clap definitions).
- [x] 55. Production deployment guide (TDX, mero-kms, forming the cluster, TLS front).
- [x] 56. Networking guide (listener, peer URLs, CORS, app domains, egress); v1's NAT/relay/bootstrap guide retired.
- [x] 57. Metrics collection and dashboard guide for `/ops/metrics`: `operate/observability` "Collecting metrics" (`--metrics-listen`, a sealing client, the fleet's Grafana dashboard and alert rules).
- [x] 58. Restore runbook, once snapshot restore exists: `operate/runbooks` "Restore an app" (`POST /ops/apps/{app}/restore`).
- [x] 59. Security guide (TDX vs --dev, quote policies, governance, ops token).
- [ ] 60. Capacity & performance tuning guide.
- [x] 61. Multi-process cluster tutorial (separate `merod` processes): `contribute/development` "Run local nodes".
- [x] 62. Document authentication (passkeys, sessions, API keys, ops token); v1's auth service and JWT providers retired.

## H · Protocol track content
- [x] 63. Identity & key-rotation chapter (`protocol/identities`, `protocol/key-rotation`).
- [x] 66. Teams and capabilities (open and restricted teams, inheritance): `protocol/organizations`.
- [ ] 69. Normalize the whole spec to a consistent normative voice.
- [ ] 71. Versioned spec snapshot mechanism.
- [x] 72. "Differences from the legacy model" note for context: `protocol/glossary` maps the peer-to-peer release's names.

## I · Contribute track content
- [ ] 73. "Where to start / good first issue" guide.
- [x] 74. Testing strategy page (unit, cluster, CLI, SDK harnesses, scenarios, the process suite).
- [ ] 75. "Add a new X" cookbook (config option, collection, host fn).
- [x] 77. Debugging guide (`merod inspect`, tracing, `ops status`): `operate/troubleshooting`, `operate/observability` "Logs", `operate/runbooks` "Inspect a stopped node".

## J · Accuracy & freshness
- [ ] 80. Reconcile `minRuntimeVersion` docs vs the bundle-manifest reality.
- [ ] 81. Sweep remaining `:::caution` markers and resolve where possible.
- [ ] 82. Cross-check every config default against a freshly generated config.
- [x] 83. Verify every node API route against `router` in `crates/node/src/api.rs`.
- [ ] 84. Add a status badge convention per chapter (stable/draft/planned).
- [ ] 85. CI drift check: generated refs vs committed.
- [ ] 86. Remove stale references to the legacy architecture site.

## K · Automation & sustain
- [ ] 87. Generate the config reference from the config structs.
- [x] 88. Generate the node API reference from OpenAPI. (`docs/public/openapi.json`, from the registry in `crates/node/src/api/openapi.rs`.)
- [ ] 89. Generate the CLI reference from clap `--help`.
- [ ] 90. Generate the SDK reference from rustdoc JSON.
- [ ] 91. CI accessibility/contrast check (axe/pa11y).
- [ ] 92. CI prose-lint/spellcheck (Vale).
- [x] 93. CI build + internal-link check on docs PRs.
- [ ] 94. Versioned docs at 1.0 (Git-branch snapshots).
- [ ] 95. Add a "report an issue with this page" link.

## L · Meta & polish
- [ ] 96. Per-page OG/Twitter meta + descriptions.
- [ ] 97. A "what's new in the docs" / changelog page.
- [ ] 98. Privacy-friendly analytics to learn what's read.
- [ ] 99. A "was this helpful?" feedback affordance.
- [ ] 100. Print/offline-friendly stylesheet + PDF export for the spec.

---

# Round 2 — content & quality (post-restructure)

A second round, reflecting the built-out site: depth, completeness, polish. Items about concepts the 1.0 node no longer has (CRDT merge, sync, the causal DAG, p2p networking) were removed.

## Build track — depth
- [ ] R1. SDK API reference generated from rustdoc (every public type/fn).
- [ ] R2. Per-collection reference pages with full method signatures + examples.
- [x] R3. Error-handling guide (`app::Result`, custom error enums, client propagation): `build/error-handling`.
- [ ] R4. Patterns & recipes: pagination, secondary indexes, per-user counters, soft-delete.
- [ ] R6. Multi-service app bundles (manifest, `service_name`, when to split).
- [ ] R7. Frontend integration (events over SSE, optimistic UI, reading state).
- [ ] R8. Local dev loop guide (`meroctl local dev`, debugging an app); the commands are in `operate/meroctl`.
- [ ] R9. "Coming from a traditional DB" pitfalls page.
- [ ] R10. A real chat-app tutorial with a frontend (beyond kv-store).

## Protocol depth
- [x] R18. Security / threat model page (trust assumptions, attacker capabilities, guarantees): `protocol/security-model`.
- [ ] R19. "How Calimero differs from blockchains / consensus systems" framing.

## Operate depth
- [x] R21. Node API reference with request/response schemas (OpenAPI); requests must be sealed, so a collection needs a sealing client.
- [ ] R22. CLI reference generated from `--help` (merod/meroctl), always current.
- [x] R23. Sample Grafana dashboard JSON: `operate/observability` links the fleet's dashboard and alert rules (in the infrastructure repository).
- [ ] R24. Kubernetes/Helm deployment guide + manifests.
- [x] R25. Tested backup/restore with RPO/RTO guidance (`crates/node/tests/restore.rs`; `operate/runbooks` "Schedule, RPO and RTO").
- [ ] R26. Capacity planning & sizing (disk per app, replicas per node, system group size).
- [x] R27. Cross-version upgrade runbook (node binary + app together): `operate/runbooks` "Upgrade the cluster's release", "Upgrade apps".
- [x] R28. Disaster recovery (losing a system-group majority; restoring an app from its snapshot): `operate/runbooks` "Disaster recovery", "Found a cluster again from its backups", "Restore an app".
- [ ] R30. Log reference (what each warn/error means, what to do).

## Reference completeness
- [ ] R31. Config reference auto-generated from structs.
- [x] R32. Host-function ABI reference (every `env::*`) as a standalone page: `build/host-functions`.
- [x] R33. Capability/role/permission matrix: `protocol/organizations` "Capabilities", `protocol/identities` "Which credential may do what", `protocol/limits` "Roles and capabilities".
- [ ] R34. Event catalog (the SSE streams' event types and payloads).
- [ ] R35. Error-code catalog (every API error code, with its route and fix).
- [ ] R36. Inline-link key glossary terms across pages.
- [x] R37. Versioning & compatibility matrix (schema/protocol/runtime versions): `operate/compatibility` "What has a version".
- [x] R38. One "limits & defaults" reference table (execution limits + wire caps): `protocol/limits`.

## Diagrams & interactivity
- [ ] R39. Interactive "life of an operation" stepper (play/pause/scrub).
- [ ] R43. A diagram legend across all pages (the palette is shared: item 22).
- [ ] R44. State-machine diagrams (upgrade gate, compute run, upgrade run).
- [ ] R45. Reduced-motion static fallbacks with step captions.
- [ ] R46. Alt text / ARIA on every diagram.

## Examples & tutorials
- [ ] R47. Example gallery with "what it teaches" tags.
- [x] R48. Collaborative text-editor guide (`Text`, `RichText`): `build/guides/collaborative-text`.
- [x] R49. Access-controlled app guide: `build/guides/access-control`.
- [x] R50. Migration tutorial: ship v2 with a schema change. (`build/ship-v2`, tested with `run_migration` against the shipped `.mpk`.)
- [ ] R51. xcall worked example (secondary-index pattern).
- [ ] R52. Blob upload/download worked example with a frontend.
- [ ] R53. "Deploy to a 3-node cluster" tutorial.
- [x] R54. Testing tutorial: unit (`TestApp`) + e2e (scenario files): `build/testing`.

## Design & UX polish
- [ ] R55. Per-page OG/social-card images (auto-generated).
- [ ] R56. Light-theme WCAG AA audit.
- [ ] R57. Tuned callout color tokens (note/tip/caution/danger).
- [ ] R58. Distinct visual style for Normative callouts.
- [ ] R59. Code blocks: filename headers + line highlighting on long samples.
- [ ] R60. Copy-link affordance on every heading.
- [ ] R61. Tuned on-page TOC for long deep-dives.
- [ ] R62. Mobile grouped-sidebar polish.
- [ ] R63. Reading-time estimates on long pages.
- [ ] R64. Home hero refinement.

## Search & findability
- [ ] R65. Tuned search (boost titles, group results by track).
- [ ] R66. `llms-full.txt` full-content export (alongside `llms.txt`).
- [ ] R67. Tag-driven "related pages" (extend the content schema).
- [ ] R68. An "all pages" index page.
- [ ] R69. Keyboard-shortcuts help modal.
- [ ] R70. Canonical URLs + per-page meta-description audit.

## Accuracy & automation
- [ ] R71. CI: generate config/CLI/API/SDK refs from source.
- [ ] R72. CI: drift check (generated vs committed).
- [ ] R73. CI: anchor-link check + prose lint (Vale).
- [ ] R74. CI: accessibility/contrast check (pa11y/axe).
- [ ] R75. "Last verified against commit X" stamp on reference pages.
- [ ] R76. Fix + close the filed code/doc issues (#2954–#2958) and update docs.
- [ ] R77. Docs CODEOWNERS + review checklist.
- [ ] R78. Asset/screenshot pipeline kept current.

## Community & process
- [ ] R79. Docs style guide (voice, terminology, capitalization).
- [ ] R80. RFC/proposal process page (how to propose protocol changes).
- [ ] R81. "What's new" / changelog surfaced in docs.
- [ ] R82. Public roadmap page.
- [ ] R83. Security policy / responsible disclosure page.
- [ ] R84. FAQ page.
- [ ] R85. Support / where-to-get-help page.
- [ ] R86. Governance + code-of-conduct links.

## Versioning & longevity
- [ ] R87. Versioned docs (snapshot per release) as 1.0 nears.
- [ ] R88. Version switcher in the header.
- [ ] R89. "Report an issue with this page" link (next to Edit).
- [ ] R90. i18n scaffolding (English-only for now).
- [ ] R91. Stable anchor-ID policy for deep links.
- [ ] R92. Deprecation-notice convention.

## Cross-cutting / meta
- [ ] R93. Clear "Concepts vs Spec" split note in the Protocol track.
- [ ] R94. Comparison page: Calimero vs other confidential-computing and app platforms.
- [ ] R95. Performance benchmarks page (real numbers).
- [ ] R96. Production-readiness checklist (app + node).
- [ ] R97. Observability cookbook (alerts, SLOs).
- [ ] R98. Data-privacy / what's-stored-where page.
- [ ] R99. "Limitations & known issues" page (honest).
- [ ] R100. "Was this helpful?" feedback widget to drive priorities.
