/**
 * App updates as the node judges them, for an app that shows its users an update and
 * sends them to approve it (core's C10: "Show updates in your app").
 *
 * The node classifies every update of an instance over its whole unit (the app and the
 * apps it created): **green** when nothing changes where data goes (same signer, no new
 * destination, same state version), else **red** with its reasons. `GET /apps/{app}/update`
 * answers it, with `available.summary` as one plain line for a banner.
 *
 * Only the instance's organization's owner and admins approve an update, and only on the
 * node's own consent page: a session opened on an app's frontend cannot approve. So an app
 * shows the update (`client.updates.get`), and its "Review" opens the node's update sheet
 * (`client.updates.openSheet`), which takes the person's passkey and sends them back.
 */

import type { ApiAnswer } from '../routes/types.js';
import type { MethodOptions } from '../routes/method.js';
import type { ConsentRequest, ConsentStarted, OpenConsentOptions } from './approvals.js';

/** `GET /apps/{app}/update`: the update available to an instance, its class and who approves it. */
export type UpdateView = ApiAnswer<'app_update'>;

/** The update an instance may move to, with the classifier's verdict and `summary`. */
export type AvailableUpdate = NonNullable<UpdateView['available']>;

/** An update policy as `GET /apps/{app}/upgrade-policy` and `GET /orgs/{org}/upgrade-policy` answer it. */
export type PolicyView = ApiAnswer<'app_policy'>;

type ConsentOf<K extends ConsentRequest['kind']> = Extract<ConsentRequest, { kind: K }>;

/** What an `install` consent request creates (`package`, `version` or `deployment`, `name`, ...). */
export type InstallSubject = ConsentOf<'install'>['subject'];

/**
 * An update policy (core's `UpgradePolicy`): `autoApply` (`never`, or `noDestinations`: a
 * green update of an app that sends no data out of Calimero applies on its own after
 * `holdHours`), `holdHours` (24 to 168) and `secondApprover` (a red update needs two owners
 * or admins). A field left out is its default.
 */
export type UpdatePolicy = NonNullable<ConsentOf<'updatePolicy'>['subject']['policy']>;

/** Whose update policy: one app's, or an organization's (every app of it without its own). */
export type PolicyTarget = { app: string } | { org: string };

/** The consent request that creates an instance on the node's install sheet; its outcome names the `appId`. */
export function installConsent(subject: InstallSubject, returnTo: string): ConsentOf<'install'> {
  return { kind: 'install', subject, returnTo };
}

/** The consent request that sets (`null`: drops) `target`'s update policy. */
export function updatePolicyConsent(
  target: PolicyTarget,
  policy: UpdatePolicy | null,
  returnTo: string,
): ConsentOf<'updatePolicy'> {
  return { kind: 'updatePolicy', subject: { ...target, policy }, returnTo };
}

/** The consent request that opens the node's updates page, for every organization the person approves updates of, or `org`. */
export function updatesPageConsent(returnTo: string, org?: string): ConsentOf<'updates'> {
  return { kind: 'updates', subject: org === undefined ? {} : { org }, returnTo };
}

/**
 * The consent request for the update `view` shows: `upgradeUnit` when the instance has apps
 * of its own (they move with it), else `upgrade`, to the available version. `null` when no
 * update is available.
 */
export function updateSheetConsent(app: string, view: UpdateView, returnTo: string): ConsentOf<'upgrade' | 'upgradeUnit'> | null {
  const available = view.available;
  if (!available) return null;
  const subject = { app, toVersion: available.version };
  return view.unit ? { kind: 'upgradeUnit', subject, returnTo } : { kind: 'upgrade', subject, returnTo };
}

/** What an in-app banner shows of an update, from {@link UpdateView}. */
export interface UpdateBannerState {
  /** Whether there is an update to show. */
  show: boolean;
  /** The version it moves to. */
  version: string | null;
  /** `green`, `red`, or `null` when the node could not classify it. */
  class: 'green' | 'red' | null;
  /** One plain line: the new destinations, "Nothing changes where your data goes", or its first reason. */
  summary: string;
  /** Whether the signed-in person may approve it (an owner or admin of the instance's organization). */
  mayApprove: boolean;
  /** Who approved it first, while it waits for a second owner or admin. */
  approvedBy: string | null;
}

/** {@link UpdateBannerState} of `view`. */
export function updateBanner(view: UpdateView): UpdateBannerState {
  const a = view.available;
  return {
    show: !!a,
    version: a?.version ?? null,
    class: a?.class ?? null,
    summary: a?.summary ?? (a ? `Version ${a.version} is available` : ''),
    mayApprove: view.mayApprove,
    approvedBy: a?.approvedBy ?? null,
  };
}

/** How {@link Updates.openSheet} (and the other pages) open the node's page. */
export interface OpenUpdateOptions extends OpenConsentOptions {
  /** Where the page sends the person back: by default this page (`location.origin + pathname`). */
  returnTo?: string;
  /**
   * The window to open the page in: pass one opened in the click handler when the call
   * does other work first. By default a pop-up is opened as the call starts (inside the
   * click, so a pop-up blocker allows it), and the page opens in this tab when there is none.
   */
  window?: Window | null;
}

/** A page opened: the node's answer, and the pop-up it opened in (`null`: this tab). */
export type OpenedUpdatePage = ConsentStarted & { window: Window | null };

/** What {@link Updates} uses of the client. */
export interface UpdatesHost {
  apps: {
    update(args: { app: string }, options?: MethodOptions): Promise<UpdateView>;
    approveUpgrade(args: { app: string; version: string; unit?: boolean }, options?: MethodOptions): Promise<ApiAnswer<'app_approve_upgrade'>>;
    policy(args: { app: string }, options?: MethodOptions): Promise<PolicyView>;
  };
  orgs: {
    policy(args: { org: string }, options?: MethodOptions): Promise<ApiAnswer<'org_policy'>>;
  };
  openConsentPage(request: ConsentRequest, options: OpenConsentOptions): Promise<ConsentStarted>;
}

const POPUP_FEATURES = 'popup,width=480,height=760';

/** A blank pop-up for the node's page, opened now (while the click still counts), or `null`. */
function blankPopup(): Window | null {
  const w = (globalThis as { window?: Window }).window;
  if (!w || typeof w.open !== 'function') return null;
  try {
    return w.open('', 'mero-consent', POPUP_FEATURES);
  } catch {
    return null;
  }
}

function hereUrl(): string {
  const loc = (globalThis as { location?: { origin: string; pathname: string } }).location;
  if (!loc) throw new Error('no page to return to; pass `returnTo`');
  return loc.origin + loc.pathname;
}

/**
 * `client.updates`: an instance's update, its first approval, the node's update sheet and
 * updates page, and update policies. Built on the generated methods (`client.apps.update`,
 * `client.apps.approveUpgrade`, `client.apps.policy`, `client.orgs.policy`) and the consent
 * page (`client.openConsentPage`).
 */
export class Updates {
  /** Update policies: read them, and change them on the node's page. */
  readonly policy: {
    /** The policy `target` follows (`GET /apps/{app}/upgrade-policy`, `GET /orgs/{org}/upgrade-policy`). */
    get(target: PolicyTarget, options?: MethodOptions): Promise<PolicyView | ApiAnswer<'org_policy'>>;
    /**
     * Set (`null`: drop) `target`'s policy on the node's page (`updatePolicy`): an owner or
     * admin of the organization approves it there with their passkey.
     */
    set(target: PolicyTarget, policy: UpdatePolicy | null, options?: OpenUpdateOptions): Promise<OpenedUpdatePage>;
  };

  constructor(private readonly host: UpdatesHost) {
    this.policy = {
      get: (target, options) =>
        'app' in target ? host.apps.policy({ app: target.app }, options) : host.orgs.policy({ org: target.org }, options),
      set: (target, policy, options = {}) => {
        const win = this.popup(options);
        return this.open(updatePolicyConsent(target, policy, options.returnTo ?? hereUrl()), win, options);
      },
    };
  }

  /** The update available to `app` (`GET /apps/{app}/update`), for whoever may read the app. */
  get(app: string, options?: MethodOptions): Promise<UpdateView> {
    return this.host.apps.update({ app }, options);
  }

  /**
   * Approve `app`'s red update first, where its policy asks for a second approver
   * (`POST /apps/{app}/upgrade-approvals`, a passkey step-up): another owner or admin then
   * applies it. `version` defaults to the available one, `unit` to whether the instance has
   * apps of its own. A session opened on an app's frontend is refused: open the sheet instead.
   */
  async approve(
    app: string,
    update: { version?: string; unit?: boolean } = {},
    options?: MethodOptions,
  ): Promise<ApiAnswer<'app_approve_upgrade'>> {
    let { version, unit } = update;
    if (version === undefined || unit === undefined) {
      const view = await this.get(app);
      version ??= view.available?.version;
      unit ??= view.unit;
      if (version === undefined) throw new Error(`no update is available for ${app}`);
    }
    return this.host.apps.approveUpgrade({ app, version, unit }, options);
  }

  /**
   * Open the node's update sheet for `app`'s available update (a pop-up by default): its
   * class and reasons, where data can go, and the approval (one, or the first or second of
   * two). Throws when no update is available. Poll the answer with `client.waitForConsent`.
   */
  async openSheet(app: string, options: OpenUpdateOptions = {}): Promise<OpenedUpdatePage> {
    const win = this.popup(options);
    try {
      const view = await this.get(app);
      const request = updateSheetConsent(app, view, options.returnTo ?? hereUrl());
      if (!request) throw new Error(`no update is available for ${app}`);
      return await this.open(request, win, options);
    } catch (e) {
      if (win && !options.window) win.close();
      throw e;
    }
  }

  /**
   * Open the node's updates page (a pop-up by default): after the person confirms with their
   * passkey there, every update waiting across the apps they approve, and their
   * organizations' policies. `org` keeps it to one organization.
   */
  manage(options: OpenUpdateOptions & { org?: string } = {}): Promise<OpenedUpdatePage> {
    const win = this.popup(options);
    return this.open(updatesPageConsent(options.returnTo ?? hereUrl(), options.org), win, options);
  }

  /** Open `install`'s sheet: what gets created, where data can go, that updates wait for approval. */
  install(subject: InstallSubject, options: OpenUpdateOptions = {}): Promise<OpenedUpdatePage> {
    const win = this.popup(options);
    return this.open(installConsent(subject, options.returnTo ?? hereUrl()), win, options);
  }

  private popup(options: OpenUpdateOptions): Window | null {
    if (options.window !== undefined) return options.window;
    if (options.navigate) return null;
    return blankPopup();
  }

  private async open(request: ConsentRequest, win: Window | null, options: OpenUpdateOptions): Promise<OpenedUpdatePage> {
    const navigate =
      options.navigate ??
      (win
        ? (url: string) => {
            win.location.href = url;
          }
        : undefined);
    try {
      const started = await this.host.openConsentPage(request, { navigate, confirm: options.confirm });
      return { ...started, window: win };
    } catch (e) {
      if (win && options.window === undefined) win.close();
      throw e;
    }
  }
}
