# Account recovery

Status: implemented (PR #4844), shaped like organization recovery (#4837, `docs/specs/org-recovery.md`) as Sandi asked: a 72-hour delay during which the account's own passkeys can cancel.

## Problem

An account acts only with its own passkeys (#4791).
A person who loses every passkey cannot sign in, and nobody can act for them: not an admin, not the operator, not a node.
Organization recovery gives an organization back to a new account. It never touches the old account, so the person's personal apps and their memberships elsewhere stay out of reach.

## What recovery does

An account may hold one **recovery code**: 160 random bits shown once as 32 Crockford base32 characters in groups of four (`calimero_identity::new_recovery_code`), read back however it is typed (`normalize_recovery_code`: case, dashes, spaces, `O`/`I`/`L` as `0`/`1`).
The code stands for an Ed25519 key, as an invite token does (`proof::recovery_signing_key`, seed `SHA-256("calimero.account-recovery-code.v1\0" || code)`).
The log keeps only the public half (`SystemState::recovery_keys`, key -> account).

Starting a recovery with the code and a new passkey spends the code and logs the new passkey as pending.
After `RECOVERY_DELAY_SECS` (72 hours, the constant organization recovery uses), that passkey finishes it: it is added to the account, every session the account had ends, every API key it made is revoked, and the new passkey is signed in.
Until then, any passkey the account still has cancels it.

## Flow

| Step | Route | Who | Proof logged | Effect at apply |
| --- | --- | --- | --- | --- |
| Make a code | `POST /auth/recovery` (412 first) | the account, unbound passkey session | passkey consent over `account:<a> / __set_recovery / { recoveryKey }` | `SetAccountRecoveryKey`: replaces any code the account had; answers the code once |
| Remove it | `DELETE /auth/recovery` (412 first) | the account | consent over `account:<a> / __remove_recovery / {}` | `RemoveAccountRecoveryKey` |
| Begin | `POST /auth/recovery/begin { code }` | anyone, signed out | none (nothing logged) | a registration challenge for the account the code names |
| Start | `POST /auth/recovery/start { challengeId, code, registration }` | anyone, signed out | the code key's signature over `recover_message(key, account, credential, at)` | `StartAccountRecovery`: the code is spent; `accountRecoveries[a] = { credential, startedAt, readyAt = startedAt + RECOVERY_DELAY_SECS }`; no passkey added, no session |
| Cancel | `POST /auth/recovery/cancel` (412 first) | the account, with a passkey it already has | consent over `account:<a> / __cancel_recovery / { credentialId }` | `CancelAccountRecovery`: the pending recovery goes; the code stays spent |
| Finish | `POST /auth/recovery/finish { accountId? }` (412 first) | the new passkey, signed out, at or after `readyAt` | the new passkey's sign-in assertion over `session / __sign_in / sign_in_args(refresh)` | `FinishAccountRecovery`: the passkey is added, every session, refresh token and API key of the account is revoked, and the new session's refresh token is recorded |

`recover_message` is `"calimero.account-recovery-start.v1\0" || key || 0 || account || 0 || credential id || 0 || SHA-256(credential public key JSON) || 0 || at`.
It names the passkey, so a node that saw the log cannot swap in its own, and it is spent once (`used_proofs`).

The finish opens a session the way every sign-in does since #4849: the challenge is `api::NewSession::sign_in_statement` (the refresh token's hash and key, the session and its expiry), the assertion is checked at apply under the pending credential (user verified), the record must name that account, that passkey and no package, and the same entry records the refresh token.
So no node can finish a recovery, or open its session, without the new passkey.

`/auth/me` shows `recoveryCode: bool` and `recoveryPending: { accountId, credentialId, startedAt, readyAt } | null`, so the account's clients can warn on every device.

## Limits and delays

- `RECOVERY_DELAY_SECS` = 72 hours (`types.rs`, shared with organization recovery), judged at the commands' own `at` (the node's `app_clock`, stamped by the node that takes the request and replicated with it), never at apply.
- One pending recovery per account: a second start is refused (`409 RECOVERY_PENDING`, also at `begin`) until a passkey cancels or the recovery finishes.
- One code per account; a code starts one recovery and is spent whether that finishes or is cancelled.
- `MAX_RECOVERY_CODE_CHARS` (80) bounds the code as typed before it is read.
- A wrong code at `start` spends the ceremony (begin again). No attempt counter: guessing 160 bits is not a threat a counter changes.
- A finish before `readyAt` answers `409 RECOVERY_NOT_READY` with `details.readyAt`; no recovery under way, `404`.
- The recovered passkey is not held to the per-account passkey cap: each recovery spends a code a passkey approved.

## Threat model

| Who | Can they recover an account? | What limits it |
| --- | --- | --- |
| The operator, a node, someone reading the log | No | The log has the code's public key and spent signatures, each over one account and one passkey; the finish needs that passkey's assertion. |
| Someone who stole the code | Yes, unless the account acts within 72 hours | Every signed-in device sees `recoveryPending`; a passkey cancels. |
| The person, every passkey lost | Yes | Begin and start with the code and a new passkey, wait 72 hours, finish. |
| The person, code and passkeys lost | No | Nobody can. |

## Migration

`SystemState::recovery_keys` and `account_recoveries` are `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`, so state from before reads with neither and writes the same bytes (`state_from_before_account_recovery_reads_and_writes_as_before`).
The new system commands are not understood by a node on an earlier release; as for every system command, a cluster runs one release.

`FinishAccountRecovery { revoke_api_keys }` is `#[serde(default)]`: the API always sets it, and a finish logged by a release before key revocation (1.0.0-rc.10) reads without it and replays as that release applied it, keeping the account's API keys, so a replica that replays it after a restart reaches the state the others reached (`a_finish_logged_before_key_revocation_replays_as_it_did`).
An account recovered on such a release revokes its keys itself (`DELETE /auth/apikeys/{keyId}`) once signed in.

## API keys

A finish revokes every API key of the account, as it ends every session: whoever took the lost passkey may also have made keys with it, and a key works without a passkey.
The machines the person runs need new keys once the recovery finishes (`an_account_recovery_revokes_the_accounts_api_keys`, and the API key in `tests/recovery.rs`).
