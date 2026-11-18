/**
 * App instances (was: contexts). An app instance runs one signed package
 * release; its id is `SHA-256(app)`. Creating one is new work, which a passed
 * release deadline refuses (`ReleaseDeadlineError`).
 */

import { useCallback } from 'react';
import type { ApiComponents, AppInfo, PackageInfo } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { confirmWith, NotConnectedError, useAppId, useAsyncAction, useResource, useSignedInClient, type MutationOptions } from './internal';

type AppAccess = ApiComponents['schemas']['Access'];

/** What `useUpgradeApp().upgrade` resolves: the upgrade's log index and outcome. */
interface AppUpgradeResult {
  index: number;
  status: string;
  error: unknown;
}

/** The app instances this account can see. Was `useContexts`. */
export function useApps(filter: { package?: string; bundle?: string } = {}) {
  const client = useSignedInClient();
  const { package: pkg, bundle } = filter;
  return useResource<AppInfo[]>(
    client
      ? async () =>
          (await client.apps.list({ package: pkg || undefined, bundle: bundle || undefined })).apps
      : null,
    [],
    [client, pkg, bundle],
  );
}

export interface CreateAppInput {
  name: string;
  /** A bundle hash, or a package release (`{ package, version? }`). */
  bundle: string | { package: string; version?: string; service?: string };
  /** The organization that owns it (default: the account). */
  org?: string;
  access?: Record<string, unknown>;
  /** Methods that need the caller's passkey on every call. */
  confirmMethods?: string[];
  initArgs?: unknown;
}

/** Create an app instance. Was `useCreateContext`. Resolves its app id. */
export function useCreateApp(options?: MutationOptions) {
  const client = useSignedInClient();
  const action = useAsyncAction(options);
  const create = useCallback(
    (input: CreateAppInput) =>
      action.run(async () => {
        if (!client) throw new NotConnectedError();
        const { name, bundle, ...rest } = input;
        const target = typeof bundle === 'string' ? { bundle } : bundle;
        const { appId } = await client.apps.create({
          name,
          ...target,
          ...rest,
          access: rest.access as AppAccess | undefined,
        });
        return appId;
      }),
    [client, action.run],
  );
  return { create, loading: action.loading, error: action.error, failure: action.failure };
}

/** Delete an app instance. Was `useDeleteContext`. */
export function useDeleteApp(options?: MutationOptions) {
  const client = useSignedInClient();
  const action = useAsyncAction(options);
  const remove = useCallback(
    (appId: string) =>
      action.run(async () => {
        if (!client) throw new NotConnectedError();
        await client.apps.delete({ app: appId });
        return true;
      }),
    [client, action.run],
  );
  return { deleteApp: remove, loading: action.loading, error: action.error, failure: action.failure };
}

/** Set an app instance's metadata. Was `useSetContextMetadata`. */
export function useSetAppMetadata(options?: MutationOptions) {
  const client = useSignedInClient();
  const action = useAsyncAction(options);
  const setMetadata = useCallback(
    (appId: string, metadata: unknown) =>
      action.run(async () => {
        if (!client) throw new NotConnectedError();
        await client.apps.metadata({ app: appId, metadata });
        return true;
      }),
    [client, action.run],
  );
  return { setMetadata, loading: action.loading, error: action.error, failure: action.failure };
}

export interface UpgradeTarget {
  version?: string;
  package?: string;
  service?: string;
  bundle?: string;
  migrate?: string;
}

/**
 * Upgrade an app instance. Was `useUpgradeGroup`. Asks for the passkey (the
 * node confirms an upgrade). A session opened on an app's own frontend cannot
 * upgrade (`403`): send the person to the consent page instead
 * (`useConsent` with `upgradeConsent`).
 */
export function useUpgradeApp(appId?: string | null, options?: MutationOptions) {
  const client = useSignedInClient();
  const { passkey } = useMero();
  const id = useAppId(appId);
  const action = useAsyncAction(options);
  const upgrade = useCallback(
    (target: UpgradeTarget) =>
      action.run(async () => {
        if (!client) throw new NotConnectedError();
        if (!id) throw new Error('No app to upgrade.');
        return (await client.apps.upgrade({ app: id, ...target }, confirmWith(passkey))) as AppUpgradeResult;
      }),
    [client, id, passkey, action.run],
  );
  return { upgrade, loading: action.loading, error: action.error, failure: action.failure };
}

/** The installed package releases. */
export function usePackages() {
  const client = useSignedInClient();
  return useResource<PackageInfo[]>(
    client ? async () => (await client.packages.list()).packages : null,
    [],
    [client],
  );
}
