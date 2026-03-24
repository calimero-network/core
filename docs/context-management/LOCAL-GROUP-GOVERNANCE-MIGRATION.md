# Migrating to local group governance

This guide supports operators moving from **chain-backed (`external`)** group policy to **`local`** (signed gossip + `group_store`), or bootstrapping **new nodes** without NEAR blocks in config.

**Primary reference:** [LOCAL-GROUP-GOVERNANCE.md](./LOCAL-GROUP-GOVERNANCE.md).

---

## 1) New nodes (recommended path)

Use init-time selection so `config.toml` has no NEAR protocol params under `[context.config]`:

```bash
merod init --group-governance local
```

This sets `group_governance = "local"` and omits NEAR `network` / `contract_id` / local signer blocks and the **relayer** entry under the context client signer. Local-only group flows do not require chain RPC.

If you later need NEAR (e.g. `join_group_context` bootstrap for unknown contexts, or chain-backed apps), merge in protocol params from a template or run a second node with `merod init` (default `external`) and copy the `[context.config]` NEAR sections manually.

---

## 2) Existing deployment: flip `external` → `local`

1. **Stop the node** (clean shutdown).
2. Edit **`config.toml`**: under **`[context]`**, set **`group_governance = "local"`**.
3. **Ensure group metadata and members already exist** on this node (from prior sync, backup restore, or gossip). Join flows that need local group state will error if metadata is missing ([join_group local path](../../crates/context/src/handlers/join_group.rs)).
4. **Restart** the node.

**Caveats**

- Nodes that relied on **chain sync** for group state must receive **`SignedGroupOp`** gossip (or ops applied out-of-band) so `group_store` stays authoritative.
- **Ordering / replay:** see §5 in [LOCAL-GROUP-GOVERNANCE.md](./LOCAL-GROUP-GOVERNANCE.md) (nonces, single-admin MVP).
- **Downgrade:** switching back to **`external`** requires valid NEAR **`[context.config]`** params and typically re-sync from chain.

---

## 3) Staying on NEAR (no migration)

No change. Keep **`group_governance = "external"`** (default) and existing **`[protocols.near]`** / relayer settings.

---

## 4) Backup and rollback

- **Backup** the node data directory (RocksDB / store path in `config.toml`) before changing governance mode or merging configs.
- **Rollback:** restore the previous `config.toml` and data snapshot; do not mix mismatched `group_store` state with a mode that expects different sources of truth without an explicit reset plan.

---

## 5) Downstream / automation

- **MDMA / Ansible / cloud init:** when generating `merod` config, pass **`--group-governance local`** or set **`context.group_governance`** in TOML to match your product SKU.
- **CI:** run **`cargo test -p calimero-context`** (and integration tests you rely on) after config or handler changes; full “no NEAR in process” builds are tracked under §11 in [LOCAL-GROUP-GOVERNANCE.md](./LOCAL-GROUP-GOVERNANCE.md).

---

## Related

- [GROUP-FEATURE-OVERVIEW.md](./GROUP-FEATURE-OVERVIEW.md) — product-level group behavior.
- [merod README](../../crates/merod/README.md) — **`merod init`** options including **`--group-governance`**.
