/**
 * The signed-in account. Was `useNodeIdentity`: there are no node identities
 * or executor keys any more, the caller is the account.
 */

import { useCallback } from 'react';
import type { PasskeyProvider } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { confirmWith, useClientAction, useResource, useSignedInClient, type MutationOptions } from './internal';

/** `GET /auth/me`: the account, its passkeys, its orgs. */
export function useAccount() {
  const client = useSignedInClient();
  const { accountId } = useMero();
  const me = useResource<Record<string, unknown> | null>(client ? () => client.auth.me() : null, null, [client, accountId]);
  return { accountId, ...me };
}

/**
 * Add a passkey (another device), remove one, or name one.
 *
 * Adding and removing are approved with a passkey the account already has
 * (the node answers `412 CONFIRM_REQUIRED`): by default the provider's
 * `passkey`, which in a browser lets the person pick any passkey they hold.
 * `add({ passkey, approver })` takes other providers when the new passkey and
 * the approving one come from different places (a security key next to the
 * platform one).
 */
export function usePasskeys(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const add = useCallback(
    (providers: { passkey?: PasskeyProvider; approver?: PasskeyProvider } = {}) =>
      withClient((c) => c.addPasskey(providers.passkey ?? passkey, providers.approver ?? passkey)),
    [withClient, passkey],
  );
  const remove = useCallback(
    (credentialId: string) =>
      withClient(async (c) => {
        await c.auth.passkeyRemove({ cred: credentialId }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  const label = useCallback(
    (credentialId: string, name: string) =>
      withClient(async (c) => {
        await c.auth.passkeyLabel({ cred: credentialId, label: name });
        return true;
      }),
    [withClient],
  );
  return { add, remove, label, ...state };
}

/** What `useApiKeys().create` takes. */
export interface CreateApiKeyInput {
  label?: string;
  /** Apps the key may call (all of the account's when left out). */
  apps?: string[];
  /** Methods the key may call. */
  methods?: string[];
  readOnly?: boolean;
  /** Let the key make calls that need a confirmation. */
  allowConfirm?: boolean;
  ttlSecs?: number;
}

/**
 * The account's API keys, for agents and servers: list them, create one
 * (approved with the passkey; needs a passkey session, not an API key) and
 * revoke one (also approved with the passkey). `create` resolves
 * `{ keyId, key }`, and the key is shown once.
 */
export function useApiKeys(options?: MutationOptions) {
  const client = useSignedInClient();
  const { passkey, accountId } = useMero();
  const keys = useResource<Array<Record<string, unknown>>>(
    client ? async () => (await client.auth.apikeyList()).keys : null,
    [],
    [client, accountId],
  );
  const { refetch } = keys;
  const { withClient, ...state } = useClientAction(options);
  const create = useCallback(
    (input: CreateApiKeyInput = {}) =>
      withClient(async (c) => {
        const created = await c.auth.apikeyCreate({ ...input }, confirmWith(passkey));
        void refetch();
        return created;
      }),
    [withClient, passkey, refetch],
  );
  const revoke = useCallback(
    (keyId: string) =>
      withClient(async (c) => {
        await c.auth.apikeyRevoke({ key: keyId }, confirmWith(passkey));
        void refetch();
        return true;
      }),
    [withClient, passkey, refetch],
  );
  return {
    keys: keys.data,
    keysLoading: keys.loading,
    keysFailure: keys.failure,
    refetch,
    create,
    revoke,
    ...state,
  };
}
