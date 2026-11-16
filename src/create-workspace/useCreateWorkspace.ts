import { useCallback, useEffect, useRef, useState } from 'react';
import { consentReturn, createOrgConsent } from '@calimero-network/mero-js';
import type { ConsentResult } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { useConsent } from '../hooks/consent';
import { useCreateOrg } from '../hooks/orgs';
import { CONSENT_RETURN_MESSAGE } from './ConsentPopupReturn';

/** What the app is told once the workspace exists. */
export interface CreatedWorkspace {
  orgId: string;
  /** The workspace's name, as the person typed it. */
  name: string;
  selfName: string;
}

export type CreatePhase = 'idle' | 'approving' | 'redirect' | 'creating';

const INTENT_KEY = 'mero-react:create-workspace-intent';
const POPUP_FEATURES = 'popup,width=480,height=720';
const NARROW_PX = 640; // at or below this width a pop-up is a poor fit: go full page
const CLOSED_POLL_MS = 400;
const WAIT_POLL_MS = 1500;
const MSG = {
  declined: 'You declined the request on your node. Nothing was created.',
  closed: 'The approval window was closed before you approved. Nothing was created.',
  expired: 'The request expired before you approved it. Nothing was created.',
  noOrg: 'Your node approved the request but named no workspace. Nothing was created.',
};

interface Intent {
  name: string;
  selfName: string;
}

function readIntent(): Intent | null {
  try {
    const v = JSON.parse(sessionStorage.getItem(INTENT_KEY) ?? 'null') as Partial<Intent> | null;
    return v && typeof v.name === 'string' && typeof v.selfName === 'string' ? { name: v.name, selfName: v.selfName } : null;
  } catch {
    return null;
  }
}

function keepIntent(intent: Intent) {
  try {
    sessionStorage.setItem(INTENT_KEY, JSON.stringify(intent));
  } catch {
    // Without session storage the return is read but cannot be finished here.
  }
}

function forgetIntent() {
  try {
    sessionStorage.removeItem(INTENT_KEY);
  } catch {
    // Nothing was kept.
  }
}

const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The new org's id from an approved result, or why there is none. */
function created(result: ConsentResult): { orgId: string } | { error: string } {
  if (result.status === 'declined') return { error: MSG.declined };
  const orgId = result.status === 'approved' && result.kind === 'createOrg' ? result.outcome?.orgId : undefined;
  return orgId ? { orgId } : { error: MSG.noOrg };
}

export interface UseCreateWorkspaceOptions {
  /** Where the node's consent page sends the person back: a page of the app that mounts `ConsentPopupReturn`. */
  returnTo: string;
  onCreated: (workspace: CreatedWorkspace) => void;
}

/**
 * Creates a workspace. A session bound to the app's frontend asks the node's
 * consent page (in a pop-up, else a full-page trip); any other session creates
 * the organization directly. Resolves through `onCreated`; `error` is the
 * node's own message, or what happened to the approval.
 */
export function useCreateWorkspace({ returnTo, onCreated }: UseCreateWorkspaceOptions) {
  const { client } = useMero();
  const consent = useConsent({ throwOnError: true });
  const orgs = useCreateOrg({ throwOnError: true });
  const [bound, setBound] = useState<boolean | null>(null);
  const [phase, setPhase] = useState<CreatePhase>('idle');
  const [error, setError] = useState<string | null>(null);
  const popup = useRef<{ win: Window; url: string | null } | null>(null);
  const stop = useRef<(() => void) | null>(null);
  const busy = useRef(false);
  const decision = useRef<((go: boolean) => void) | null>(null);

  useEffect(() => {
    if (!client) return;
    let live = true;
    client.auth.me().then(
      (me) => live && setBound(typeof (me as { app?: unknown }).app === 'string' && !!(me as { app: string }).app),
      () => live && setBound(false),
    );
    return () => {
      live = false;
    };
  }, [client]);
  useEffect(() => () => stop.current?.(), []);

  const approveInPopup = async (win: Window, name: string, selfName: string) => {
    if (!client) return;
    const started = await consent.open(createOrgConsent(name, returnTo), {
      navigate: (url) => {
        if (popup.current) popup.current.url = url;
        win.location.href = url;
      },
      confirm: () => true,
    });
    if (!started) return;
    let cause: 'closed' | 'returned' | 'cancelled' | null = null;
    const ctrl = new AbortController();
    const end = (why: NonNullable<typeof cause>) => {
      cause ??= why;
      ctrl.abort();
    };
    const timer = setInterval(() => win.closed && end('closed'), CLOSED_POLL_MS);
    const onMessage = (e: MessageEvent) => {
      const data = e.data as { type?: unknown } | null;
      if (e.origin === window.location.origin && e.source === win && data?.type === CONSENT_RETURN_MESSAGE) end('returned');
    };
    window.addEventListener('message', onMessage);
    stop.current = () => end('cancelled');
    try {
      let result = await client.waitForConsent(started.requestId, { intervalMs: WAIT_POLL_MS, signal: ctrl.signal, expiresAt: started.expiresAt });
      if (result.status === 'pending' && (cause === 'closed' || cause === 'returned')) {
        result = (await client.consent.pageResult({ request: started.requestId })) as ConsentResult;
      }
      if (result.status === 'pending') {
        if (cause !== 'cancelled') setError(cause === 'closed' ? MSG.closed : MSG.expired);
        return;
      }
      const made = created(result);
      if ('error' in made) setError(made.error);
      else onCreated({ orgId: made.orgId, name, selfName });
    } finally {
      clearInterval(timer);
      window.removeEventListener('message', onMessage);
      stop.current = null;
    }
  };

  const redirect = async (name: string, selfName: string) => {
    keepIntent({ name, selfName });
    let went = false;
    try {
      await consent.open(createOrgConsent(name, returnTo), {
        // The page opens once the person has seen the code and chosen to go.
        confirm: async () => {
          setPhase('redirect');
          went = await new Promise<boolean>((resolve) => (decision.current = resolve));
          decision.current = null;
          if (went) setPhase('creating');
          return went;
        },
      });
    } catch (e) {
      forgetIntent();
      throw e;
    }
    if (!went) forgetIntent();
    // Going: the page unloads and nothing more happens here.
    else await new Promise<void>(() => undefined);
  };

  const create = (name: string, selfName: string) => {
    if (busy.current || bound === null) return;
    busy.current = true;
    setError(null);
    // Opened inside the click so a pop-up blocker allows it.
    const win = bound && window.innerWidth > NARROW_PX ? window.open('', 'mero-consent', POPUP_FEATURES) : null;
    if (win) popup.current = { win, url: null };
    setPhase(win ? 'approving' : 'creating');
    void (async () => {
      try {
        if (!bound) {
          const orgId = await orgs.create(name);
          if (orgId) onCreated({ orgId, name, selfName });
        } else if (win) await approveInPopup(win, name, selfName);
        else await redirect(name, selfName);
      } catch (e) {
        setError(messageOf(e));
      } finally {
        if (win && !win.closed) win.close();
        popup.current = null;
        busy.current = false;
        setPhase('idle');
      }
    })();
  };

  const cancel = useCallback(() => {
    stop.current?.();
    decision.current?.(false);
  }, []);
  const proceed = useCallback(() => decision.current?.(true), []);
  const reopen = useCallback(() => {
    const p = popup.current;
    if (!p || p.win.closed) return;
    if (p.url) p.win.location.href = p.url;
    p.win.focus();
  }, []);

  return { create, cancel, reopen, proceed, phase, error, code: consent.code, ready: bound !== null };
}

/**
 * Finishes a workspace the person approved on a full-page trip to the node:
 * call it where the app starts, with the same `onCreated`. `message` says why
 * nothing was created (declined, or a return that is not this tab's request).
 */
export function useCreateWorkspaceResume({ onCreated }: Pick<UseCreateWorkspaceOptions, 'onCreated'>) {
  const { client } = useMero();
  const [message, setMessage] = useState<string | null>(null);
  const started = useRef(false);
  const latest = useRef(onCreated);
  latest.current = onCreated;

  useEffect(() => {
    if (!client || started.current || !consentReturn(window.location.href)) return;
    const intent = readIntent();
    if (!intent) return;
    started.current = true;
    void (async () => {
      try {
        const result = await client.resumeConsent();
        if (!result) return;
        const made = created(result);
        if ('error' in made) setMessage(made.error);
        else latest.current({ orgId: made.orgId, name: intent.name, selfName: intent.selfName });
      } catch (e) {
        setMessage(messageOf(e));
      } finally {
        forgetIntent();
      }
    })();
  }, [client]);

  return { message };
}
