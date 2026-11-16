import { acceptInviteConsent, createInviteConsent } from '@calimero-network/mero-js';
import type { ConsentResult, OrgRole } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { confirmWith } from '../hooks/internal';
import { inviteLink } from '../hooks/invites';
import { useConsentResume, useNodeConsent } from '../consent-flow/useNodeConsent';

export type InviteRole = Exclude<OrgRole, 'owner'>;

/** An invite just created. */
export interface CreatedInvite {
  token: string;
  /** The link to share: the token in the fragment of `linkBase`. */
  link: string;
  /** The capability bits it grants; absent for the workspace's member defaults. */
  capabilities?: number;
}

const INVITE_KEY = 'mero-react:invite-intent';
const ACCEPT_KEY = 'mero-react:accept-invite-intent';
const NO_TOKEN = 'Your node approved the request but made no invite. Nothing was changed.';
const noOrg = (noun: string) => `Your node approved the request but named no ${noun}. Nothing was changed.`;

interface InviteIntent {
  role: InviteRole;
  capabilities?: number;
  /** Kept over a full-page trip so the shared link still opens the page the app chose. */
  linkBase?: string;
}

const isInviteIntent = (v: unknown): v is InviteIntent => !!v && typeof (v as InviteIntent).role === 'string';
const isObject = (v: unknown): v is Record<string, never> => !!v && typeof v === 'object';

const tokenOf = (r: ConsentResult) => (r.status === 'approved' && r.kind === 'createInvite' ? r.outcome?.token : undefined);
const orgOf = (r: ConsentResult) => (r.status === 'approved' && r.kind === 'acceptInvite' ? r.outcome?.org : undefined);

export interface InviteRequest {
  org: string;
  role: InviteRole;
  teams?: string[];
  /** How many people may use the link. */
  uses?: number;
  /** How long the link lives, in seconds. */
  ttlSecs?: number;
  /** Every app of the org, not just the named teams. */
  allApps?: boolean;
  /** Only for a custom invite; left out for the workspace's member defaults. */
  capabilities?: number;
}

/** Creates an invite: through the node's consent page for a session bound to the app, else directly. */
export function useInviteCreate({ returnTo, linkBase, onCreated }: { returnTo: string; linkBase?: string; onCreated: (invite: CreatedInvite) => void }) {
  const { client, passkey } = useMero();
  const { start, ...flow } = useNodeConsent<InviteIntent>({ storageKey: INVITE_KEY });
  const finish = (token: string | undefined, capabilities: number | undefined) => {
    if (!token) return NO_TOKEN;
    onCreated({ token, link: inviteLink(token, linkBase), ...(capabilities === undefined ? {} : { capabilities }) });
    return undefined;
  };
  const create = ({ org, role, teams, uses, ttlSecs, allApps, capabilities }: InviteRequest) => {
    const terms = {
      ...(teams?.length ? { teams } : {}),
      ...(uses === undefined ? {} : { uses }),
      ...(ttlSecs === undefined ? {} : { ttlSecs }),
      ...(allApps === undefined ? {} : { allApps }),
      ...(capabilities === undefined ? {} : { capabilities }),
    };
    return start({
      request: createInviteConsent({ org, role, ...terms }, returnTo),
      intent: { role, ...(capabilities === undefined ? {} : { capabilities }), ...(linkBase ? { linkBase } : {}) },
      direct: async () => {
        if (!client) throw new Error('Not connected to a node.');
        const answer = await client.orgs.inviteCreate({ org, role, ...terms }, confirmWith(passkey));
        return answer.token;
      },
      done: (outcome) => finish('direct' in outcome ? outcome.direct : tokenOf(outcome.consent), capabilities),
    });
  };
  return { create, ...flow };
}

/** Finishes an invite approved on a full-page trip: hands its token to `onCreated` and says why nothing was made. */
export function useInviteResume({ linkBase, onCreated }: { linkBase?: string; onCreated: (invite: CreatedInvite) => void }) {
  return useConsentResume<InviteIntent>({
    storageKey: INVITE_KEY,
    valid: isInviteIntent,
    onApproved: (intent, result) => {
      const token = tokenOf(result);
      if (!token) return NO_TOKEN;
      onCreated({ token, link: inviteLink(token, intent.linkBase ?? linkBase), ...(intent.capabilities === undefined ? {} : { capabilities: intent.capabilities }) });
      return undefined;
    },
  });
}

/** Joins with an invite token: through the node's consent page for a session bound to the app, else directly. */
export function useInviteAccept({ returnTo, noun, onJoined, onError }: { returnTo: string; noun: string; onJoined: (joined: { org: string }) => void; onError?: (error: unknown) => void }) {
  const ctx = useMero();
  const { start, ...flow } = useNodeConsent<Record<string, never>>({ storageKey: ACCEPT_KEY, onError });
  const accept = (token: string) =>
    start({
      request: acceptInviteConsent(token, returnTo),
      intent: {},
      direct: async () => {
        if (!ctx.client) throw new Error('Not connected to a node.');
        return ((await ctx.client.orgs.inviteRedeem({ token })) as { org?: string }).org;
      },
      done: (outcome) => {
        const org = 'direct' in outcome ? outcome.direct : orgOf(outcome.consent);
        if (!org) return noOrg(noun);
        onJoined({ org });
        return undefined;
      },
    });
  return { accept, ...flow };
}

export function useAcceptResume({ noun, onJoined }: { noun: string; onJoined: (joined: { org: string }) => void }) {
  return useConsentResume<Record<string, never>>({
    storageKey: ACCEPT_KEY,
    valid: isObject,
    onApproved: (_intent, result) => {
      const org = orgOf(result);
      if (!org) return noOrg(noun);
      onJoined({ org });
      return undefined;
    },
  });
}
