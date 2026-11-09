# Organization recovery

Status: implemented (PR #4837). The three choices below were agreed with Sandi.

## Problem

An organization's owner is an account, and an account acts only with its own passkeys.
An owner who loses every passkey can no longer act for the organization, and nobody can act for them:
not an admin (only the owner changes ownership), not the operator (no ops route touches an organization), not a node (every decision carries its maker's proof, checked at apply, #4791).
The organization is stuck for good.

## What recovery does

Each organization may hold a **recovery key**: an Ed25519 key whose 32-byte seed the owner keeps offline (printed, in a password manager).
The node stores only the public half, on the organization.

Presenting the seed moves the organization's **ownership** to an account the person signs up with a new passkey, after a delay during which the current owner can cancel.
The old owner account is removed from that organization, so its passkeys and sessions stop reaching it.

Recovery acts on the organization, never on an account's credentials:
an account's passkeys still change only with one of its own passkeys (#4791), and one organization's seed reaches nothing outside that organization (not the owner's other organizations, not their personal apps).
An account that owns several organizations sets a key on each.

## Flow

| Step | Route | Who | Proof logged | Effect at apply |
| --- | --- | --- | --- | --- |
| Set or rotate | `POST /orgs/{org}/recovery { publicKey }` (`null` removes) | the owner | passkey consent over `org:<org> / __set_recovery_key / { publicKey }` | replaces the key; a pending recovery is cancelled |
| Set at creation | `POST /orgs { name, recoveryKey }` | the creator | the session (as creating an organization today) | the new organization holds the key |
| Start | `POST /orgs/{org}/recovery/start { nonce, signature }` | any signed-in account (the new one) | the seed's signature over `recovery_message(org, account, nonce)` | `pending = { account, startedAt, readyAt = startedAt + RECOVERY_DELAY_SECS }` |
| Cancel | `POST /orgs/{org}/recovery/cancel` | the owner | passkey consent over `__cancel_recovery { account }` | `pending = null` |
| Finish | `POST /orgs/{org}/recovery/finish` | the pending account, at or after `readyAt` | passkey consent over `__finish_recovery { account }` | the account becomes owner; every previous owner is removed from the organization and its teams |

`recovery_message` is `"calimero.org-recovery.v1\0" || org || 0 || account || 0 || nonce` (`calimero_identity::recovery_message`).
The signature binds the account it hands the organization to, so a node that saw the log cannot redirect it to another account, and it is spent once (`used_proofs`).
The signature is checked against the organization's **current** key, at the API and again where the command applies, on every node.

`GET /orgs/{org}` shows members `recovery: { publicKey, setAt, pending }` (absent when no key is set), so a client can warn every member, the owner first, while a recovery is pending.
The recovering account is not a member yet; it reads the pending recovery's times in the answers of `start` and `finish`.

## Limits and delays

- `RECOVERY_DELAY_SECS` = 72 hours between start and finish, judged at the commands' own times (stamped by the node that takes the request, replicated with it), never at apply.
- One pending recovery per organization: a second start is refused (`409 RECOVERY_PENDING`) until the owner cancels or the pending one finishes. A seed holder cannot replace a pending recovery with their own.
- `MAX_RECOVERY_NONCE_BYTES` (64) bounds the nonce; `MAX_RECOVERY_KEY_CHARS` and `MAX_RECOVERY_SIGNATURE_CHARS` bound the key and signature text before decoding.
- No per-node attempt counter: a wrong seed is refused at the API before anything is logged, and guessing a 256-bit seed is not a threat a counter changes. The general request limits apply.

## What is logged

Every step is a system-log entry with its proof: the key's public half and who set it, the start (the account, the nonce, the signature, the time), a cancel (the owner's consent), the finish (the new owner's consent).
The organization's `recovery` field keeps the public key, when it was set, and the pending recovery; members read it.
Nothing about recovery is in the Ops API or `/ops/metrics`: the operator learns no more than it does about any organization.

## Threat model

| Who | Can they recover an organization? | Why not / what limits it |
| --- | --- | --- |
| The operator | No | The seed never reaches a node: the client derives the public key and signs locally. No ops route starts, cancels or finishes a recovery, and the operator's signature is not a recovery proof. |
| A node (or someone reading the log) | No | The log holds the public key and spent signatures, each bound to one account. A node cannot sign for another account, nor replay a signature (`used_proofs`). |
| An admin or member | No | Setting, rotating and cancelling need the owner's passkey; starting needs the seed. |
| A non-member account | Only with the seed | The seed is the credential; anyone holding it can start. |
| Someone who stole the seed | Yes, unless the owner acts within 72 hours | Every member sees the pending recovery; the owner cancels with a passkey and rotates the key. |
| The owner, all passkeys lost | Yes | Sign up with a new passkey, start with the seed, wait 72 hours, finish. |

## If the seed leaks

The owner (with a passkey) rotates: `POST /orgs/{org}/recovery { publicKey: <new> }`.
Rotation cancels a pending recovery and makes every signature under the old key fail, at the API and at apply.
A thief who already finished a recovery owns the organization; the delay and the warning to members are the defence, as with any account recovery.

## If the owner lost both

An owner without a passkey and without the seed cannot recover; neither can anyone else.
This is the property we keep: no party but the seed holder can take an organization.

## Migration

Organizations written before this change have no key (`Org::recovery` is `#[serde(default, skip_serializing_if = "Option::is_none")]`), so a snapshot from before reads with none and writes the same bytes (`state_from_before_recovery_reads_with_none_and_writes_as_before`).
Their owners set one later with the owner route; until then `start` answers `403 WRONG_RECOVERY_KEY`, as for a wrong seed or an unknown organization, so an outsider learns nothing about which organizations exist or hold a key.
The new system commands are not understood by a node on an earlier release; as for every system command, a cluster runs one release.

## Choices

- Optional, not mandatory: offered at creation (`recoveryKey`) and settable later. A mandatory key would break creating an organization from every client until it ships.
- Recovery moves ownership instead of adding a passkey to the owner's account: one organization's seed then never reaches the account's other organizations or personal apps, and account credentials keep their one rule.
- 72 hours: long enough to notice over a weekend, short enough to recover a working organization within a working week.

## Not covered

Recovering data when every mero-kms replica is lost (backups opened by the organization's own key, k of n custodians) is the "Rollback protection and org recovery" design; the recovery key here could later be one of those custodians, but this change does not wrap any backup to it.
