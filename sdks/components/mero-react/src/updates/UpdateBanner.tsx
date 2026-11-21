import type { ConsentResult } from '@calimero-network/mero-js';
import { useUpdate } from './useUpdate';

/** The banner's words, each replaceable. */
export interface UpdateBannerLabels {
  /** "Update {version} is ready". */
  ready?: (version: string) => string;
  review?: string;
  reviewing?: string;
  /** Shown in place of "Review" to a member who may not approve. */
  notYours?: string;
  green?: string;
  red?: string;
}

export interface UpdateBannerProps {
  /** The app instance; the provider's `appId` when left out. */
  app?: string | null;
  labels?: UpdateBannerLabels;
  className?: string;
  /** After the node's page answered (approved or declined). */
  onReviewed?: (result: ConsentResult) => void;
}

const DEFAULTS: Required<UpdateBannerLabels> = {
  ready: (v) => `Update ${v} is ready`,
  review: 'Review',
  reviewing: 'Waiting for your node…',
  notYours: 'An owner or admin approves updates',
  green: 'Green',
  red: 'Red',
};

/**
 * Screen 4 of the update design: one line inside the app when its instance has an update,
 * with the node's class and summary ("Update 1.4 is ready · Nothing changes where your data
 * goes"). "Review" opens the node's own update sheet in a pop-up, where an owner or admin
 * approves with their passkey; a member who may not approve sees who does instead. Renders
 * nothing while there is no update. Mount `ConsentPopupReturn` at the app's root so the
 * pop-up closes itself when the node sends it back.
 */
export function UpdateBanner({ app, labels, className, onReviewed }: UpdateBannerProps) {
  const l = { ...DEFAULTS, ...labels };
  const { banner, review, reviewing, reviewError } = useUpdate(app);
  if (!banner?.show || !banner.version) return null;
  const cls = banner.class ?? 'none';
  return (
    <div className={['mero-update-banner', `mero-update-banner--${cls}`, className].filter(Boolean).join(' ')} role="status">
      {banner.class && <span className={`mero-update-badge mero-update-badge--${cls}`}>{banner.class === 'green' ? l.green : l.red}</span>}
      <span className="mero-update-text">
        <strong>{l.ready(banner.version)}</strong>
        {banner.summary && <span> · {banner.summary}</span>}
        {reviewError && <span className="mero-update-error"> · {reviewError.message}</span>}
      </span>
      {banner.mayApprove ? (
        <button
          type="button"
          className="mero-update-review"
          disabled={reviewing}
          onClick={() => {
            void review().then((r) => r && onReviewed?.(r));
          }}
        >
          {reviewing ? l.reviewing : l.review}
        </button>
      ) : (
        <span className="mero-update-who">{l.notYours}</span>
      )}
    </div>
  );
}
