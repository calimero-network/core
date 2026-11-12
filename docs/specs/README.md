# Design specs

Design records: the problem, the options weighed and the decision. They are not the
documentation of current behaviour, which is the docs site (`docs/src/content/docs/`) and
each crate's `AGENTS.md`; code comments cite these files for the reasoning behind a rule.
Keep each file's **Status** line true when its work lands.

| Spec | Status |
| --- | --- |
| [app-runtime-v0.14.md](app-runtime-v0.14.md) | The `calimero:app` runtime and SDK spec; the authority the app-engine crates cite (`crates/app-wit`, `app-host`, `app-engine-wasmtime`, `sdk`, `sdk-macros`, `app-build`). |
| [org-recovery.md](org-recovery.md) | Implemented (#4837). Docs: protocol "Organizations". |
| [account-recovery.md](account-recovery.md) | Implemented (#4844). Docs: protocol "Identities". |
| [rollback-protection.md](rollback-protection.md) | Rollback protection implemented; keys per app not yet. |
| [custom-domains.md](custom-domains.md) | Core part (served origins) implemented; other parts belong to mero-tee and mdma. |
| [gate-system-group.md](gate-system-group.md) | Phase 0 implemented; phases 1 to 3 proposed. |
| [org-owned-deployments.md](org-owned-deployments.md) | Largely implemented; migration, transfer and lockout guard not done. |
