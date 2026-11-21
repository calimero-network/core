import { useCallback, useEffect, useRef, useState } from 'react';
import { updateBanner, updateSheetConsent } from '@calimero-network/mero-js';
import type { ConsentResult, UpdateBannerState, UpdateView } from '@calimero-network/mero-js';
import { useAppId, useResource, useSignedInClient, type ResourceState } from '../hooks/internal';

const POPUP_FEATURES = 'popup,width=480,height=760';
const CLOSED_POLL_MS = 400;
const WAIT_POLL_MS = 1500;

export interface UseUpdateResult extends ResourceState<UpdateView | null> {
  /** What a banner shows of it: `show`, `version`, `class`, `summary`, `mayApprove`, `approvedBy`. `null` before the first read. */
  banner: UpdateBannerState | null;
  /**
   * Open the node's update sheet for it in a pop-up (call it from the click, so a pop-up
   * blocker allows it; this tab goes to the page when none opens), wait for the decision and
   * read the update again. Answers the node's result, or `null` when the window closed or
   * the request expired undecided.
   */
  review: () => Promise<ConsentResult | null>;
  /** Whether the sheet is open now. */
  reviewing: boolean;
  /** Why the last review did not reach the sheet (no update, the node refused). */
  reviewError: Error | null;
}

/**
 * The update available to an app instance (`appId`, else the provider's) as the node judges
 * it (`GET /apps/{app}/update`): green or red, a one-line `summary`, and whether the signed-in
 * person may approve it. Only the node's page approves: `review()` opens it.
 */
export function useUpdate(appId?: string | null): UseUpdateResult {
  const client = useSignedInClient();
  const app = useAppId(appId);
  const resource = useResource<UpdateView | null>(
    client && app ? () => client.apps.update({ app }) : null,
    null,
    [client, app],
  );
  const [reviewing, setReviewing] = useState(false);
  const [reviewError, setReviewError] = useState<Error | null>(null);
  const stop = useRef<(() => void) | null>(null);
  useEffect(() => () => stop.current?.(), []);
  const { data, refetch } = resource;

  const review = useCallback(async (): Promise<ConsentResult | null> => {
    if (!client || !app || !data) return null;
    const win = typeof window !== 'undefined' ? window.open('', 'mero-consent', POPUP_FEATURES) : null;
    setReviewing(true);
    setReviewError(null);
    const ctrl = new AbortController();
    const timer = win ? setInterval(() => win.closed && ctrl.abort(), CLOSED_POLL_MS) : null;
    stop.current = () => ctrl.abort();
    try {
      const request = updateSheetConsent(app, data, window.location.origin + window.location.pathname);
      if (!request) throw new Error('no update is available');
      const started = await client.openConsentPage(request, {
        navigate: (url) => {
          if (win) win.location.href = url;
          else window.location.assign(url);
        },
      });
      if (!win) return null;
      const result = await client.waitForConsent(started.requestId, {
        intervalMs: WAIT_POLL_MS,
        signal: ctrl.signal,
        expiresAt: started.expiresAt,
      });
      await refetch();
      return result.status === 'pending' ? null : result;
    } catch (e) {
      setReviewError(e instanceof Error ? e : new Error(String(e)));
      return null;
    } finally {
      if (timer) clearInterval(timer);
      if (win && !win.closed) win.close();
      stop.current = null;
      setReviewing(false);
    }
  }, [client, app, data, refetch]);

  return { ...resource, banner: data ? updateBanner(data) : null, review, reviewing, reviewError };
}
