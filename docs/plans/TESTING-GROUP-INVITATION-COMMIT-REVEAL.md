# Complete testing process: Group invitation commit/reveal flow

End-to-end testing for the group invitation flow where the **joiner** adds themselves on-chain via commit → reveal (no admin action at join time).

---

## 1. Prerequisites

- **Contracts built** (see step 2).
- **NEAR testnet account** with enough NEAR (e.g. 10+ NEAR for deploy and multiple group/join txs).
  - Create at [wallet.testnet.near.org](https://wallet.testnet.near.org) or use `near login`.
  - Get testnet NEAR from the wallet or [faucet](https://near.org/faucet/).
- **merod (core) rebuilt** and runnable (you rebuild this yourself).
- **near-cli** installed: [docs.near.org/tools/near-cli](https://docs.near.org/tools/near-cli).

---

## 2. Build contracts

From the **contracts** workspace (repo root that contains the contracts workspace):

```bash
cd /path/to/Calimero/contracts
./scripts/build-rust.sh
```

This builds:

- `contracts/near/context-config/res/calimero_context_config_near.wasm`
- `contracts/near/context-proxy/res/calimero_context_proxy_near.wasm`

---

## 3. Deploy contracts to testnet

3.1 **Create a testnet account** (if you don’t have one for the contract):

```bash
# Use your main testnet account as master
near create-account my-context-config.testnet --masterAccount YOUR_ACCOUNT.testnet --initialBalance 10
```

3.2 **Deploy and set proxy code**:

```bash
cd /path/to/Calimero/contracts
export CONTRACT_ACCOUNT=my-context-config.testnet
./scripts/deploy-context-config-testnet.sh
```

The script deploys context-config and calls `set_proxy_code` with the context-proxy WASM.  
Note **CONTRACT_ACCOUNT**; you’ll use it as `contract_id` in merod/meroctl.

---

## 4. Run contract unit tests (optional)

From the contracts workspace:

```bash
cd /path/to/Calimero/contracts
cargo test -p calimero-context-config-near
```

Covers context-config (including group invitation logic in sandbox).  
Group invitation–specific tests live in the same crate (e.g. commit/reveal paths if added there).

---

## 5. Start merod against the deployed contract

Rebuild merod, then start the node and admin API with **protocol=near**, **network_id=testnet**, and **contract_id** = the account you deployed (e.g. `my-context-config.testnet`).  
Use your normal config (env or config file) so that:

- Admin API is reachable (e.g. `http://localhost:PORT/admin-api`).
- Context config client uses: `protocol=near`, `network_id=testnet`, `contract_id=CONTRACT_ACCOUNT`.

---

## 6. Two identities (admin + joiner)

You need two identities (keypairs) and, for each, a way to sign group requests:

- **Admin**: used to create the group and the invitation; must have a **signing key** registered for the group (so the node can sign `mutate` for create_group, add_members, etc.).
- **Joiner**: used to join via invitation; must have a **signing key** for the group (so the node can sign **commit_group_invitation** and **reveal_group_invitation**).

If you use meroctl with a local keystore, create or import two identities and register a signing key per identity for the group (see steps 8 and 9).

---

## 7. Create group (admin)

Using the **admin** identity and the group ID you want:

```bash
meroctl group create <GROUP_ID_HEX> \
  --requester <ADMIN_PUBLIC_KEY> \
  --app-key <APP_KEY> \
  --target-application-id <TARGET_APP_ID> \
  --target-application-blob <TARGET_APP_BLOB>
```

Or call the admin API:

```bash
curl -X POST "http://localhost:PORT/admin-api/groups" \
  -H "Content-Type: application/json" \
  -d '{
    "groupId": "<GROUP_ID_HEX>",
    "requester": "<ADMIN_PUBLIC_KEY>",
    "appKey": "<APP_KEY>",
    "targetApplicationId": "<TARGET_APP_ID>",
    "targetApplicationBlob": "<TARGET_APP_BLOB>"
  }'
```

Then **register the admin’s signing key** for this group so the node can sign group requests:

```bash
meroctl group signing-key register <GROUP_ID_HEX> \
  --identity <ADMIN_PUBLIC_KEY> \
  --signing-key <ADMIN_SIGNING_KEY_HEX>
```

Or:

```bash
curl -X POST "http://localhost:PORT/admin-api/groups/<GROUP_ID>/signing-key" \
  -H "Content-Type: application/json" \
  -d '{ "identity": "<ADMIN_PUBLIC_KEY>", "signingKey": "<ADMIN_SIGNING_KEY_HEX>" }'
```

---

## 8. Create group invitation (admin)

Admin creates an invitation (optional: invitee, expiration). The server returns a **payload** string (base58).

**meroctl:**

```bash
meroctl group invite <GROUP_ID_HEX> \
  --requester <ADMIN_PUBLIC_KEY> \
  [--invitee-identity <INVITEE_PUBLIC_KEY>] \
  [--expiration <UNIX_TIMESTAMP>]
```

**API:**

```bash
curl -X POST "http://localhost:PORT/admin-api/groups/<GROUP_ID>/invite" \
  -H "Content-Type: application/json" \
  -d '{
    "requester": "<ADMIN_PUBLIC_KEY>",
    "inviteeIdentity": "<INVITEE_PUBLIC_KEY or null>",
    "expiration": <UNIX_TIMESTAMP or null>
  }'
```

Save the returned **payload** for the joiner.

---

## 9. Joiner: register signing key (if not already)

The joiner must have a signing key for the group so the node can sign **commit_group_invitation** and **reveal_group_invitation**. Register it before joining (or pass it in the join request if your API supports it):

```bash
meroctl group signing-key register <GROUP_ID_HEX> \
  --identity <JOINER_PUBLIC_KEY> \
  --signing-key <JOINER_SIGNING_KEY_HEX>
```

Or:

```bash
curl -X POST "http://localhost:PORT/admin-api/groups/<GROUP_ID>/signing-key" \
  -H "Content-Type: application/json" \
  -d '{ "identity": "<JOINER_PUBLIC_KEY>", "signingKey": "<JOINER_SIGNING_KEY_HEX>" }'
```

---

## 10. Joiner: join group with invitation payload

Using the **invitation payload** from step 8 and the **joiner** identity:

**meroctl:**

```bash
meroctl group join "<INVITATION_PAYLOAD>" \
  --joiner-identity <JOINER_PUBLIC_KEY>
```

**API:**

```bash
curl -X POST "http://localhost:PORT/admin-api/groups/join" \
  -H "Content-Type: application/json" \
  -d '{
    "invitationPayload": "<INVITATION_PAYLOAD>",
    "joinerIdentity": "<JOINER_PUBLIC_KEY>"
  }'
```

Success means:

- The node ran commit (if used) and reveal on the context-config contract.
- The contract verified inviter (admin) and joiner signatures and added the joiner to the group.
- The node updated local group store.

---

## 11. Verify: list group members

**meroctl:**

```bash
meroctl group list-members <GROUP_ID_HEX>
```

**API:**

```bash
curl "http://localhost:PORT/admin-api/groups/<GROUP_ID>/members"
```

You should see both the **admin** and the **joiner** (and any other members). The joiner was added on-chain via **reveal_group_invitation**, not via admin `add_group_members`.

---

## 12. Quick checklist

| Step | Action |
|------|--------|
| 1 | Get testnet NEAR; create/use account |
| 2 | `contracts/scripts/build-rust.sh` |
| 3 | `CONTRACT_ACCOUNT=... ./scripts/deploy-context-config-testnet.sh` |
| 4 | (Optional) `cargo test -p calimero-context-config-near` |
| 5 | Start merod with `contract_id=<CONTRACT_ACCOUNT>`, testnet |
| 6 | Have admin + joiner identities and signing keys |
| 7 | Create group (admin); register admin signing key for group |
| 8 | Create invitation (admin); save payload |
| 9 | Register joiner signing key for group |
| 10 | Join group (joiner) with payload |
| 11 | List members; confirm joiner is member |

---

## Troubleshooting

- **“only group admins can add members”**  
  You’re not using the new flow. Ensure merod uses the **rebuilt** code and the **deployed** contract that has `commit_group_invitation` / `reveal_group_invitation`. The joiner must call join (commit + reveal), not `add_group_members`.

- **“No matching commitment found”**  
  Commit step failed or expired. Ensure commit is sent before reveal and that expiration (block height) is in the future.

- **“Inviter's signature is invalid” / “Inviter is not a group admin”**  
  Invitation was created by someone who isn’t a group admin, or the payload was altered. Create the invitation with the same admin that created the group.

- **“New member's signature is invalid”**  
  Joiner identity in the reveal payload doesn’t match the key that signed the payload. Ensure the joiner’s signing key is the one used by the node for that identity and group.

- **Joiner has no signing key**  
  Register the joiner’s signing key for the group (step 9) before joining, or pass it in the join request if your client supports it.
