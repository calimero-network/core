import fs from 'node:fs';
import path from 'node:path';
import { chromium, expect } from '@playwright/test';
import { SCREENSHOTS, SUITE, ports, runDir } from './env.mjs';
import { bump } from './counter.mjs';

const MAX_PROMPT_DELAY_MS = 1000; // later than this after its click, a browser may refuse a passkey prompt

// Chromium's Local Network Access check would stop a `<label>.localhost` page from calling the node.
const FLAGS =
  '--disable-features=LocalNetworkAccessChecks,PrivateNetworkAccessForNavigations,BlockInsecurePrivateNetworkRequests';
export const launch = () => chromium.launch({ headless: true, args: [FLAGS] });

const scripts = () =>
  fs.readFileSync(path.join(runDir(), 'mero.iife.js'), 'utf8') +
  '\n' +
  fs.readFileSync(path.join(SUITE, 'support/browser/soft-passkey.js'), 'utf8');

/**
 * A browser context for `account`: a virtual authenticator holding the account's passkey
 * (the file meroctl made, imported with CDP), and one page. The returned object is the
 * handle every helper below takes.
 */
export async function person(browser, account) {
  const ctx = await browser.newContext({ viewport: { width: 520, height: 900 }, deviceScaleFactor: 2 });
  const page = await ctx.newPage();
  const cdp = await ctx.newCDPSession(page);
  await cdp.send('WebAuthn.enable');
  const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
    options: {
      protocol: 'ctap2',
      transport: 'internal',
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  const pk = JSON.parse(fs.readFileSync(account.file, 'utf8'));
  const b64 = (s) => Buffer.from(s, 'base64url').toString('base64');
  await cdp.send('WebAuthn.addCredential', {
    authenticatorId,
    credential: {
      credentialId: b64(pk.credentialId),
      isResidentCredential: true,
      rpId: 'localhost',
      privateKey: b64(pk.pkcs8),
      userHandle: Buffer.from('consent-page-test').toString('base64'),
      signCount: bump(5),
    },
  });
  return { ctx, page, cdp, authenticatorId, account };
}

/** Put the authenticator's counter ahead of everything signed so far (reserving a block for this page). */
export async function advanceCounter(p) {
  const next = bump(1);
  bump(10);
  const { credentials } = await p.cdp.send('WebAuthn.getCredentials', { authenticatorId: p.authenticatorId });
  for (const c of credentials) {
    await p.cdp.send('WebAuthn.removeCredential', {
      authenticatorId: p.authenticatorId,
      credentialId: c.credentialId,
    });
    await p.cdp.send('WebAuthn.addCredential', {
      authenticatorId: p.authenticatorId,
      credential: { ...c, signCount: next },
    });
  }
}

/**
 * The app's own page: `http://<label>.localhost:<port>/1.0.0/`, which the test serves itself
 * (the node would serve the release's frontend) so the origin is the package's real one.
 * Chromium refuses the RP id `localhost` from that origin, so the page signs in with a
 * software passkey for the same account; the consent page, on the node's own origin, uses
 * the real authenticator.
 */
export async function appPage(p, { label, node = 2 }) {
  const port = ports()[node - 1];
  const origin = `http://${label}.localhost:${port}`;
  await p.ctx.route(`${origin}/1.0.0/**`, (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: `<!doctype html><title>app</title><body><script>${scripts()}</script></body>`,
    }),
  );
  const app = { p, origin, port, back: `${origin}/1.0.0/` };
  await signInOnApp(app);
  return app;
}

async function signInOnApp({ p, back, port }) {
  await p.page.goto(back);
  const pk = JSON.parse(fs.readFileSync(p.account.file, 'utf8'));
  await p.page.evaluate(
    async ({ pk, counter, port }) => {
      const c = new Mero.CalimeroClient({ baseUrl: `http://127.0.0.1:${port}`, policy: { allowMock: true } });
      await c.signIn(SoftApp(pk, location.origin, counter));
      window.c = c;
    },
    { pk, counter: bump(), port },
  );
}

/** Ask the node for a consent page as the app does. The url is on `localhost`, the passkey's RP id. */
export async function askConsent(app, request) {
  if (!(await app.p.page.evaluate(() => !!window.c).catch(() => false))) await signInOnApp(app);
  const made = await app.p.page.evaluate((body) => window.c.consent.pageCreate({ body }), {
    ...request,
    returnTo: app.back,
  });
  return { ...made, url: made.url.replace('//127.0.0.1:', '//localhost:') };
}

/** Open a consent page; answer the model the node sent it and the text the person reads. */
export async function openSheet(p, made, screenshot) {
  await p.page.goto(made.url);
  await p.page.waitForSelector('#content:not([hidden]), #status:not(:empty)');
  const model = await p.page.evaluate(async () => {
    const id = decodeURIComponent(location.pathname.split('/')[2] || '');
    return (await fetch(`/consent/${encodeURIComponent(id)}/model`)).json();
  });
  if (screenshot) {
    fs.mkdirSync(SCREENSHOTS, { recursive: true });
    await p.page.screenshot({ path: path.join(SCREENSHOTS, `${screenshot}.png`), fullPage: true });
  }
  return { model, text: await p.page.evaluate(() => document.body.innerText) };
}

/** Approve with the passkey and wait for the page to send the person back to the app. */
export async function approve(p) {
  await advanceCounter(p);
  await p.page.click('#approve');
  await p.page.waitForURL(/\.localhost:\d+\//);
  return p.page.url();
}

/**
 * A client on the cluster's registered passkey origin (`--origins`), signed in with the real
 * authenticator: `window.n` in the page. For what has no consent sheet (invites, public links,
 * organization settings), which the owner's passkey approves on the API.
 */
export async function nodeClient(p, node = 1) {
  const origin = 'http://localhost:3000';
  await p.ctx.route(`${origin}/__client`, (route) =>
    route.fulfill({
      status: 200,
      contentType: 'text/html',
      body: `<!doctype html><title>client</title><body><script>${scripts()}</script></body>`,
    }),
  );
  await p.page.goto(`${origin}/__client`);
  await advanceCounter(p);
  await p.page.evaluate(
    async (base) => {
      const n = new Mero.CalimeroClient({ baseUrl: base, policy: { allowMock: true } });
      await n.signIn(Mero.browserPasskey());
      window.n = n;
      window.confirm1 = () => ({ confirm: Mero.passkeyConfirm(Mero.browserPasskey()) });
    },
    `http://localhost:${ports()[node - 1]}`,
  );
}

/** Run `fn(arg)` in the node-origin page, with the authenticator's counter moved ahead first. */
export async function signedCall(p, fn, arg) {
  await advanceCounter(p);
  return p.page.evaluate(fn, arg);
}

/**
 * Record every passkey prompt `p`'s pages ask: ms since the last trusted click, and the requests
 * sent in between. The first `refuse` prompts fail at once with `NotAllowedError`, as a browser
 * refuses one asked too long after its click.
 */
export async function guardPrompts(p, { refuse = 0 } = {}) {
  const calls = [];
  await p.ctx.exposeFunction('__passkeyCall', (c) => calls.push(c));
  await p.ctx.addInitScript(recordPrompts, refuse);
  return calls;
}

/** Every prompt in `calls` was asked on its click: soon enough, and with no request in between. */
export function expectPromptsOnClick(calls, count) {
  expect(calls).toHaveLength(count);
  for (const c of calls) {
    expect(c.requests, `requests between "${c.click}" and its passkey prompt`).toEqual([]);
    expect(c.delayMs, `ms from "${c.click}" to its passkey prompt`).toBeLessThanOrEqual(MAX_PROMPT_DELAY_MS);
  }
}

function recordPrompts(refuse) {
  let click = null;
  document.addEventListener(
    'click',
    (e) => {
      if (!e.isTrusted) return;
      const el = e.target instanceof Element ? e.target.closest('button') : null;
      click = { at: performance.now(), label: (el?.textContent || '').trim(), requests: [] };
    },
    true,
  );
  const note = (method, url) => {
    if (click)
      click.requests.push(
        `${(method || 'GET').toUpperCase()} ${new URL(String(url), location.href).pathname}`,
      );
  };
  const fetch0 = window.fetch;
  window.fetch = function (input, init) {
    note(init?.method || input?.method, input?.url ?? input);
    return fetch0.apply(this, arguments);
  };
  const open0 = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    note(method, url);
    return open0.apply(this, arguments);
  };
  for (const kind of ['create', 'get']) {
    const call = CredentialsContainer.prototype[kind];
    CredentialsContainer.prototype[kind] = function (options) {
      if (!options?.publicKey) return call.call(this, options);
      window.__passkeyCall({
        kind,
        click: click?.label ?? null,
        delayMs: click ? Math.round(performance.now() - click.at) : null,
        requests: click ? [...click.requests] : [],
      });
      if (refuse > 0) {
        refuse -= 1;
        return Promise.reject(new DOMException('The request is not allowed.', 'NotAllowedError'));
      }
      return call.call(this, options);
    };
  }
}
