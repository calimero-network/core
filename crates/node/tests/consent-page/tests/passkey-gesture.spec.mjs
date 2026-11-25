import { expect, test } from '@playwright/test';
import {
  advanceCounter,
  appPage,
  approve,
  askConsent,
  expectPromptsOnClick,
  guardPrompts,
  launch,
  openSheet,
  person,
} from '../support/browser.mjs';
import { newAccount } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

// Browsers show a passkey prompt only close to the click that asks for it (Safari strictest), so the
// page holds the node's challenge before the click and asks the authenticator inside it.
const pkg = 'com.test.gesture';
let owner;

test.beforeAll(() => {
  owner = newAccount('owner');
  install(owner, { pkg, version: '1.0.0', tag: 'gesture' });
});

const challenges = (page) => {
  const seen = [];
  page.on('response', async (r) => {
    if (r.request().method() === 'POST' && r.url().endsWith('/approve') && r.status() === 412) {
      seen.push((await r.json()).details.challengeId);
    }
  });
  return seen;
};

/** An install sheet for `name`, open in a guarded browser and done loading, as a person finds it. */
async function sheet(browser, name, { refuse = 0, clock = false } = {}) {
  const me = await person(browser, owner);
  const calls = await guardPrompts(me, { refuse });
  const app = await appPage(me, { label: label(pkg) });
  const made = await askConsent(app, { kind: 'install', subject: { package: pkg, version: '1.0.0', name } });
  if (clock) await me.page.clock.install();
  const seen = challenges(me.page);
  await openSheet(me, made);
  await me.page.waitForLoadState('networkidle');
  return { me, calls, seen };
}

test('the passkey prompt is asked on the approve click, with no request to the node first', async () => {
  const browser = await launch();
  try {
    const { me, calls } = await sheet(browser, 'on-click');
    expect(await approve(me)).toContain('result=approved');
    expectPromptsOnClick(calls, 1);
  } finally {
    await browser.close();
  }
});

test('a prompt the browser refuses unseen is asked again by one more click', async () => {
  const browser = await launch();
  try {
    const { me, calls } = await sheet(browser, 'refused', { refuse: 1 });
    await advanceCounter(me);
    await me.page.click('#approve');
    await expect(me.page.locator('#approve')).toHaveText('Confirm with your passkey');
    await expect(me.page.locator('#status')).toContainText('one more tap');
    expect(await approve(me)).toContain('result=approved');
    expectPromptsOnClick(calls, 2);
  } finally {
    await browser.close();
  }
});

test('a held challenge is asked afresh before the node forgets it', async () => {
  const browser = await launch();
  try {
    const { me, calls, seen } = await sheet(browser, 'refreshed', { clock: true });
    await expect.poll(() => seen.length).toBe(1);
    await me.page.clock.fastForward('04:30');
    await expect.poll(() => seen.length).toBe(2);
    const confirmed = me.page.waitForRequest(
      (r) => r.url().endsWith('/approve') && !!r.postDataJSON()?.confirm,
    );
    expect(await approve(me)).toContain('result=approved');
    expect((await confirmed).postDataJSON().confirm.challengeId).toBe(seen[1]);
    expectPromptsOnClick(calls, 1);
  } finally {
    await browser.close();
  }
});

test('a challenge the node no longer holds is asked again, then one more click approves', async () => {
  const browser = await launch();
  try {
    const { me, calls, seen } = await sheet(browser, 'expired');
    await expect.poll(() => seen.length).toBe(1);
    let answered = false;
    await me.page.route('**/approve', (route) => {
      if (answered || !route.request().postDataJSON()?.confirm) return route.continue();
      answered = true;
      return route.fulfill({
        status: 400,
        contentType: 'application/json',
        body: JSON.stringify({ status: 400, error: 'EXPIRED_CHALLENGE', message: 'the challenge expired' }),
      });
    });
    await advanceCounter(me);
    await me.page.click('#approve');
    await expect(me.page.locator('#approve')).toHaveText('Confirm with your passkey');
    await expect.poll(() => seen.length).toBe(2);
    expect(await approve(me)).toContain('result=approved');
    expectPromptsOnClick(calls, 2);
  } finally {
    await browser.close();
  }
});
