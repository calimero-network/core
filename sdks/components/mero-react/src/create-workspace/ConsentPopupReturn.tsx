import { useEffect, useSyncExternalStore, type ReactNode } from 'react';
import { consentReturn } from '@calimero-network/mero-js';

/** The `postMessage` type the consent pop-up sends its opener when the node sends it back. */
export const CONSENT_RETURN_MESSAGE = 'mero:consent-return';

type Returned = ReturnType<typeof consentReturn>;

/** What the consent page put on this URL, when this window is a same-origin pop-up opened by the app. */
function popupReturn(): Returned {
  if (typeof window === 'undefined' || !window.opener) return null;
  try {
    if (window.opener.location.origin !== window.location.origin) return null;
    return consentReturn(window.location.href);
  } catch {
    return null;
  }
}

const never = () => () => undefined;

/**
 * Mount at the app's root. In the pop-up the node's consent page sends back to
 * the app, it tells the opener (which confirms with the node) and closes the
 * window, rendering nothing; anywhere else it renders `children`.
 */
export function ConsentPopupReturn({ children }: { children?: ReactNode }) {
  const returned = useSyncExternalStore(never, () => JSON.stringify(popupReturn()), () => 'null');
  useEffect(() => {
    const value = JSON.parse(returned) as Returned;
    if (!value || !window.opener) return;
    window.opener.postMessage({ type: CONSENT_RETURN_MESSAGE, ...value }, window.location.origin);
    window.close();
  }, [returned]);
  return returned === 'null' ? <>{children}</> : null;
}
