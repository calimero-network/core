/**
 * Invitations: an org admin mints a token (`POST /orgs/{org}/invites`), the
 * invitee redeems it while signed in (`POST /invites/{token}/redeem`) and
 * becomes a member of the org (and of the teams the invite names).
 *
 * There is no relay, no node to join and no sync to wait for any more: the
 * cluster holds the membership, so the redeem's answer is the whole join.
 *
 * Links carry the token in the URL fragment (`#invite=…`), which browsers do
 * not send to servers, so it stays out of access logs.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { useMero } from '../context';
import { describeError, type Failure } from '../errors';
import { confirmWith, useClientAction, type MutationOptions } from './internal';
import type { OrgRole } from '@calimero-network/mero-js';

const PARAM = 'invite';
const STASH_KEY = 'mero:pendingInvite';

/** A link to `base` (default: this page) that carries an invite token. */
export function inviteLink(token: string, base?: string): string {
  const url = new URL(base ?? (typeof window !== 'undefined' ? window.location.href : 'http://localhost/'));
  const hash = new URLSearchParams(url.hash.slice(1));
  hash.set(PARAM, token);
  url.hash = hash.toString();
  return url.toString();
}

/** The invite token in a URL's fragment or query, or `null`. */
export function readInviteToken(href: string): string | null {
  try {
    const url = new URL(href);
    return new URLSearchParams(url.hash.slice(1)).get(PARAM) ?? url.searchParams.get(PARAM);
  } catch {
    return null;
  }
}

/** Remove the invite token from the address bar, keeping everything else. */
function stripInviteFromLocation(): void {
  if (typeof window === 'undefined') return;
  const url = new URL(window.location.href);
  const hash = new URLSearchParams(url.hash.slice(1));
  hash.delete(PARAM);
  url.searchParams.delete(PARAM);
  url.hash = hash.toString();
  window.history.replaceState(window.history.state, '', url.toString());
}

function session(): Storage | null {
  try {
    return typeof window !== 'undefined' ? window.sessionStorage : null;
  } catch {
    return null;
  }
}

export interface CreateInviteInput {
  orgId: string;
  role?: Exclude<OrgRole, 'owner'>;
  /** Teams the invitee joins too. */
  teams?: string[];
  /** How long the token lives. */
  ttlSecs?: number;
  /** How many times it can be redeemed. */
  uses?: number;
  /** The page the link opens (default: this one). */
  linkBase?: string;
}

/**
 * Mint an invitation (asks for the passkey). Was `useCreateNamespaceInvitation`.
 * Resolves `{ token, link }`. Redeeming it needs no passkey.
 */
export function useCreateInvite(options?: MutationOptions) {
  const { passkey } = useMero();
  const { withClient, ...state } = useClientAction(options);
  const create = useCallback(
    ({ orgId, role = 'member', teams = [], ttlSecs, uses, linkBase }: CreateInviteInput) =>
      withClient(async (c) => {
        const { token } = await c.orgs.inviteCreate({ org: orgId, role, teams, ttlSecs, uses }, confirmWith(passkey));
        return { token, link: inviteLink(token, linkBase) };
      }),
    [withClient, passkey],
  );
  return { create, ...state };
}

/** Redeem a token. Was `useRedeemInvitation` / `useJoinNamespace` / `useJoinGroup`. */
export function useRedeemInvite(options?: MutationOptions) {
  const { withClient, ...state } = useClientAction(options);
  const redeem = useCallback(
    (token: string) => withClient(async (c) => (await c.orgs.inviteRedeem({ token })) as Record<string, unknown>),
    [withClient],
  );
  return { redeem, ...state };
}

export type InviteRedemptionStatus = 'idle' | 'waiting-for-sign-in' | 'redeeming' | 'redeemed' | 'failed';

export interface InviteRedemption {
  /** The token this page was opened with (or kept from before a sign-in). */
  token: string | null;
  status: InviteRedemptionStatus;
  /** What the node answered on a redeem. */
  result: Record<string, unknown> | null;
  failure: Failure | null;
  /** Redeem now (also after a failure). */
  redeem: () => Promise<void>;
  /** Forget the token. */
  dismiss: () => void;
}

/**
 * The invite link flow in one hook: read the token from the address bar (and
 * take it out of it), keep it across the sign-in, redeem it once signed in.
 * Same name as before; there is no relay or join-sync step any more.
 */
export function useInviteRedemption({ autoRedeem = true }: { autoRedeem?: boolean } = {}): InviteRedemption {
  const { isAuthenticated, client, reportError } = useMero();
  const [token, setToken] = useState<string | null>(() => {
    if (typeof window === 'undefined') return null;
    return readInviteToken(window.location.href) ?? session()?.getItem(STASH_KEY) ?? null;
  });
  const [status, setStatus] = useState<InviteRedemptionStatus>('idle');
  const [result, setResult] = useState<Record<string, unknown> | null>(null);
  const [failure, setFailure] = useState<Failure | null>(null);
  const attempted = useRef<string | null>(null);

  useEffect(() => {
    if (!token) return;
    try {
      session()?.setItem(STASH_KEY, token);
    } catch {
      /* kept in memory only */
    }
    stripInviteFromLocation();
  }, [token]);

  const redeem = useCallback(async () => {
    if (!token) return;
    if (!client || !isAuthenticated) {
      setStatus('waiting-for-sign-in');
      return;
    }
    attempted.current = token;
    setStatus('redeeming');
    setFailure(null);
    try {
      const answer = (await client.orgs.inviteRedeem({ token })) as Record<string, unknown>;
      setResult(answer);
      setStatus('redeemed');
      session()?.removeItem(STASH_KEY);
    } catch (error) {
      reportError(error);
      setFailure(describeError(error));
      setStatus('failed');
    }
  }, [token, client, isAuthenticated, reportError]);

  useEffect(() => {
    if (!token) return;
    if (!isAuthenticated) {
      setStatus('waiting-for-sign-in');
      return;
    }
    if (autoRedeem && attempted.current !== token) void redeem();
  }, [token, isAuthenticated, autoRedeem, redeem]);

  const dismiss = useCallback(() => {
    session()?.removeItem(STASH_KEY);
    setToken(null);
    setStatus('idle');
    setResult(null);
    setFailure(null);
  }, []);

  return { token, status, result, failure, redeem, dismiss };
}
