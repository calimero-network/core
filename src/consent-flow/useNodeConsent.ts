import { useCallback, useEffect, useRef, useState } from 'react';
import { consentReturn } from '@calimero-network/mero-js';
import type { ConsentRequest, ConsentResult } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { useConsent } from '../hooks/consent';
import { CONSENT_RETURN_MESSAGE } from '../create-workspace/ConsentPopupReturn';

export type NodeConsentPhase = 'idle' | 'approving' | 'redirect' | 'creating';

const POPUP_FEATURES = 'popup,width=480,height=720';
const NARROW_PX = 640; // at or below this width a pop-up is a poor fit: go full page
const CLOSED_POLL_MS = 400;
const WAIT_POLL_MS = 1500;
export const CONSENT_MSG = {
  declined: 'You declined the request on your node. Nothing was changed.',
  closed: 'The approval window was closed before you approved. Nothing was changed.',
  expired: 'The request expired before you approved it. Nothing was changed.',
};

export const messageOf = (e: unknown) => (e instanceof Error ? e.message : String(e));
const codeOf = (e: unknown) => (typeof (e as { code?: unknown } | null)?.code === 'string' ? (e as { code: string }).code : null);

function readIntent<I>(key: string, valid: (v: unknown) => v is I): I | null {
  try {
    const v: unknown = JSON.parse(sessionStorage.getItem(key) ?? 'null');
    return valid(v) ? v : null;
  } catch {
    return null;
  }
}

function keepIntent(key: string, intent: unknown) {
  try {
    sessionStorage.setItem(key, JSON.stringify(intent));
  } catch {
    // Without session storage the return is read but cannot be finished here.
  }
}

function forgetIntent(key: string) {
  try {
    sessionStorage.removeItem(key);
  } catch {
    // Nothing was kept.
  }
}

/** What a finished request made: the direct call's answer, or the node's approved result. */
export type NodeOutcome<R> = { direct: R } | { consent: ConsentResult };

export interface StartOptions<I, R> {
  request: ConsentRequest;
  /** Kept in `sessionStorage` over a full-page trip. */
  intent: I;
  /** What an unbound session does itself, with no consent step. */
  direct: () => Promise<R>;
  /** Takes what was made; returns a message to show when it made nothing usable. */
  done: (outcome: NodeOutcome<R>) => string | void;
}

/**
 * Something only the node's consent page may do for a session bound to an
 * app's frontend: the pop-up (or, when it cannot open, a full-page trip) with
 * the code, the wait, and the ways it ends. An unbound session does `direct`.
 */
export function useNodeConsent<I>({ storageKey, onError }: { storageKey: string; onError?: (error: unknown) => void }) {
  const { client } = useMero();
  const consent = useConsent({ throwOnError: true });
  const [bound, setBound] = useState<boolean | null>(null);
  const [phase, setPhase] = useState<NodeConsentPhase>('idle');
  const [error, setError] = useState<{ message: string; code: string | null } | null>(null);
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

  const approveInPopup = async (win: Window, request: ConsentRequest): Promise<ConsentResult | null> => {
    if (!client) return null;
    const started = await consent.open(request, {
      navigate: (url) => {
        if (popup.current) popup.current.url = url;
        win.location.href = url;
      },
      confirm: () => true,
    });
    if (!started) return null;
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
        if (cause !== 'cancelled') setError({ message: cause === 'closed' ? CONSENT_MSG.closed : CONSENT_MSG.expired, code: null });
        return null;
      }
      if (result.status === 'declined') {
        setError({ message: CONSENT_MSG.declined, code: null });
        return null;
      }
      return result;
    } finally {
      clearInterval(timer);
      window.removeEventListener('message', onMessage);
      stop.current = null;
    }
  };

  const redirect = async (request: ConsentRequest, intent: I) => {
    keepIntent(storageKey, intent);
    let went = false;
    try {
      await consent.open(request, {
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
      forgetIntent(storageKey);
      throw e;
    }
    if (!went) forgetIntent(storageKey);
    // Going: the page unloads and nothing more happens here.
    else await new Promise<void>(() => undefined);
  };

  const start = <R,>({ request, intent, direct, done }: StartOptions<I, R>) => {
    if (busy.current || bound === null) return;
    busy.current = true;
    setError(null);
    // Opened inside the click so a pop-up blocker allows it.
    const win = bound && window.innerWidth > NARROW_PX ? window.open('', 'mero-consent', POPUP_FEATURES) : null;
    if (win) popup.current = { win, url: null };
    setPhase(win ? 'approving' : 'creating');
    void (async () => {
      try {
        let outcome: NodeOutcome<R> | null = null;
        if (!bound) outcome = { direct: await direct() };
        else if (win) {
          const result = await approveInPopup(win, request);
          if (result) outcome = { consent: result };
        } else await redirect(request, intent);
        const problem = outcome && done(outcome);
        if (problem) setError({ message: problem, code: null });
      } catch (e) {
        setError({ message: messageOf(e), code: codeOf(e) });
        onError?.(e);
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
  const clearError = useCallback(() => setError(null), []);

  return {
    start,
    cancel,
    reopen,
    proceed,
    clearError,
    phase,
    error: error?.message ?? null,
    errorCode: error?.code ?? null,
    code: consent.code,
    ready: bound !== null,
    bound,
  };
}

/**
 * Finishes what the person approved on a full-page trip to the node: reads the
 * result from the node and hands an approved one, with the kept intent, to
 * `onApproved`. Does nothing without this flow's kept intent. `message` says
 * why nothing happened (declined, or a return that is not this tab's request).
 */
export function useConsentResume<I>({
  storageKey,
  valid,
  onApproved,
}: {
  storageKey: string;
  valid: (v: unknown) => v is I;
  onApproved: (intent: I, result: ConsentResult) => string | void;
}) {
  const { client } = useMero();
  const [message, setMessage] = useState<string | null>(null);
  const started = useRef(false);
  const latest = useRef({ valid, onApproved });
  latest.current = { valid, onApproved };

  useEffect(() => {
    if (!client || started.current || !consentReturn(window.location.href)) return;
    const intent = readIntent(storageKey, latest.current.valid);
    if (!intent) return;
    started.current = true;
    void (async () => {
      try {
        const result = await client.resumeConsent();
        if (!result) return;
        if (result.status === 'declined') setMessage(CONSENT_MSG.declined);
        else if (result.status === 'pending') setMessage(CONSENT_MSG.expired);
        else setMessage(latest.current.onApproved(intent, result) ?? null);
      } catch (e) {
        setMessage(messageOf(e));
      } finally {
        forgetIntent(storageKey);
      }
    })();
  }, [client, storageKey]);

  return { message };
}
