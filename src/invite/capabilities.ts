import { ORG_CAPABILITIES, type OrgCapability } from '@calimero-network/mero-js';

/** What each capability bit lets a member do, in the words the dialog shows. */
export const CAPABILITY_LABELS: ReadonlyArray<{ name: OrgCapability; label: string }> = [
  { name: 'CREATE_APP', label: 'Add apps' },
  { name: 'INVITE_MEMBERS', label: 'Invite people' },
  { name: 'JOIN_OPEN_TEAMS', label: 'Join open teams' },
  { name: 'MANAGE_MEMBERS', label: 'Manage team members' },
  { name: 'MANAGE_APPLICATION', label: 'Upgrade apps' },
  { name: 'CREATE_TEAM', label: 'Create teams' },
  { name: 'CREATE_CHILD_TEAM', label: 'Create child teams' },
  { name: 'DELETE_TEAM', label: 'Delete teams' },
  { name: 'MANAGE_VISIBILITY', label: 'Change who can join a team' },
  { name: 'MANAGE_METADATA', label: 'Rename the workspace and its teams' },
  { name: 'APPROVE_RELEASES', label: 'Approve app releases' },
];

export const ALL_CAPABILITIES = Object.values(ORG_CAPABILITIES).reduce((mask, bit) => mask | bit, 0);

/** The labels of the bits set in `mask`, in display order. */
export function describeCapabilities(mask: number): string[] {
  return CAPABILITY_LABELS.filter(({ name }) => (mask & ORG_CAPABILITIES[name]) !== 0).map(({ label }) => label);
}

/** The bits an account may give away: everything for an owner or admin, else its own row (the workspace default when it has none). */
export function heldCapabilities(org: { members: Record<string, string>; capabilities: Record<string, number>; defaultCapabilities: number }, account: string | null): number {
  const role = account ? org.members[account] : undefined;
  if (role === 'owner' || role === 'admin') return ALL_CAPABILITIES;
  return account ? (org.capabilities[account] ?? org.defaultCapabilities) : 0;
}
