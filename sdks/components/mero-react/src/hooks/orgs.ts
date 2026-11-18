/**
 * Organizations (was: namespaces) and teams (was: groups).
 *
 * An organization holds members (guest, member, admin, owner), teams and the
 * apps it owns. A team is a set of an org's members, optionally nested under
 * another team, joined by anyone in the org (`everyone`) or by invite only.
 */

import { useCallback } from 'react';
import type { OrgInfo, OrgRole, TeamRole } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { confirmWith, useClientAction, useResource, useSignedInClient, type MutationOptions } from './internal';

export type TeamJoin = 'everyone' | 'invite';

// ---------------------------------------------------------------- organizations

/** An organization: members, teams, metadata, release approvers. Was `useNamespace`. */
export function useOrg(orgId?: string | null) {
  const client = useSignedInClient();
  // The document marks `releaseApprovers`, `publishes` and `newWorkStopped`
  // optional (absent from older nodes); `OrgInfo` declares them present.
  return useResource<OrgInfo | null>(
    client && orgId ? async () => (await client.orgs.get({ org: orgId })) as OrgInfo : null,
    null,
    [client, orgId],
  );
}

/** Create an organization; resolves its id. Was `useCreateNamespace`. */
export function useCreateOrg(options?: MutationOptions) {
  const { withClient, ...state } = useClientAction(options);
  const create = useCallback(
    (name: string) => withClient(async (c) => (await c.orgs.create({ name })).orgId),
    [withClient],
  );
  return { create, ...state };
}

/** Delete an organization (asks for the passkey). Was `useDeleteNamespace`. */
export function useDeleteOrg(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const deleteOrg = useCallback(
    (orgId: string) =>
      withClient(async (c) => {
        await c.orgs.delete({ org: orgId }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  return { deleteOrg, ...state };
}

/**
 * Add a member or change their role (asks for the passkey). Was
 * `useUpdateMemberRole` and, at the org level, `useAddGroupMembers`.
 */
export function useSetMember(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const setMember = useCallback(
    (orgId: string, account: string, role: OrgRole) =>
      withClient(async (c) => {
        await c.orgs.setMember({ org: orgId, account, role }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  return { setMember, ...state };
}

/**
 * Remove a member from an organization, or leave it (`account` your own).
 * Asks for the passkey.
 */
export function useRemoveMember(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const removeMember = useCallback(
    (orgId: string, account: string) =>
      withClient(
        async (c) =>
          (await c.orgs.removeMember({ org: orgId, account }, confirmWith(passkey))) as Record<string, unknown>,
      ),
    [withClient, passkey],
  );
  return { removeMember, ...state };
}

/** Set an organization's metadata. Was `useSetGroupMetadata` on a namespace root. */
export function useSetOrgMetadata(options?: MutationOptions) {
  const { withClient, ...state } = useClientAction(options);
  const setMetadata = useCallback(
    (orgId: string, metadata: unknown) =>
      withClient(async (c) => {
        await c.orgs.metadata({ org: orgId, metadata });
        return true;
      }),
    [withClient],
  );
  return { setMetadata, ...state };
}

/** Set a member's metadata (a display name, say). Same name as before; now per org. */
export function useSetMemberMetadata(options?: MutationOptions) {
  const { withClient, ...state } = useClientAction(options);
  const setMetadata = useCallback(
    (orgId: string, account: string, metadata: unknown) =>
      withClient(async (c) => {
        await c.orgs.memberMetadata({ org: orgId, account, metadata });
        return true;
      }),
    [withClient],
  );
  return { setMetadata, ...state };
}

/**
 * Set capability bits (`ORG_CAPABILITIES`) for an account or a team, or the
 * org's default with neither. Asks for the passkey. Was
 * `useSetDefaultCapabilities`.
 */
export function useSetCapabilities(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const setCapabilities = useCallback(
    (orgId: string, capabilities: number, at: { account?: string; team?: string } = {}) =>
      withClient(async (c) => {
        await c.orgs.capabilities({ org: orgId, capabilities, ...at }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  return { setCapabilities, ...state };
}

// ---------------------------------------------------------------- teams

/** A team of an organization. Was `useGroupInfo`. */
export function useTeam(orgId?: string | null, teamId?: string | null) {
  const client = useSignedInClient();
  return useResource<Record<string, unknown> | null>(
    client && orgId && teamId ? () => client.orgs.teamGet({ org: orgId, team: teamId }) : null,
    null,
    [client, orgId, teamId],
  );
}

export interface CreateTeamInput {
  orgId: string;
  name: string;
  join?: TeamJoin;
  /** The team it sits under. */
  parent?: string;
}

/** Create a team; resolves its id. Was `useCreateGroupInNamespace`. */
export function useCreateTeam(options?: MutationOptions) {
  const { withClient, ...state } = useClientAction(options);
  const create = useCallback(
    ({ orgId, name, join = 'invite', parent }: CreateTeamInput) =>
      withClient(async (c) => (await c.orgs.teamCreate({ org: orgId, name, join, parent })).teamId),
    [withClient],
  );
  return { create, ...state };
}

export interface TeamUpdate {
  name?: string;
  join?: TeamJoin;
  /** Move it under another team; `null` makes it top-level. */
  parent?: string | null;
  metadata?: unknown;
}

/**
 * Rename, re-parent, change who may join, or set metadata. Was
 * `useSetGroupMetadata`, `useReparentGroup` and `useSetSubgroupVisibility`.
 */
export function useUpdateTeam(options?: MutationOptions) {
  const { withClient, ...state } = useClientAction(options);
  const update = useCallback(
    (orgId: string, teamId: string, change: TeamUpdate) =>
      withClient(async (c) => {
        await c.orgs.teamUpdate({ org: orgId, team: teamId, ...change });
        return true;
      }),
    [withClient],
  );
  return { update, ...state };
}

/**
 * Delete a team, the teams below it and their apps. Was `useDeleteGroup`.
 * Asks for the passkey only when one of those teams holds apps (the node
 * answers `412 CONFIRM_REQUIRED`); a team with no apps goes without one.
 */
export function useDeleteTeam(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const deleteTeam = useCallback(
    (orgId: string, teamId: string) =>
      withClient(async (c) => {
        await c.orgs.teamDelete({ org: orgId, team: teamId }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  return { deleteTeam, ...state };
}

/**
 * Add an org member to a team, or change their row (asks for the passkey).
 * Was `useAddGroupMembers`.
 */
export function useSetTeamMember(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const setTeamMember = useCallback(
    (
      orgId: string,
      teamId: string,
      account: string,
      row: { role?: TeamRole; lead?: boolean; capabilities?: number } = {},
    ) =>
      withClient(async (c) => {
        await c.orgs.teamSetMember({ org: orgId, team: teamId, account, ...row }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  return { setTeamMember, ...state };
}

/** Remove someone from a team (asks for the passkey). Was `useRemoveGroupMembers`. */
export function useRemoveTeamMember(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const removeTeamMember = useCallback(
    (orgId: string, teamId: string, account: string) =>
      withClient(async (c) => {
        await c.orgs.teamRemoveMember({ org: orgId, team: teamId, account }, confirmWith(passkey));
        return true;
      }),
    [withClient, passkey],
  );
  return { removeTeamMember, ...state };
}
