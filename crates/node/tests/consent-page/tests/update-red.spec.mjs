import { expect, test } from '@playwright/test';
import {
  advanceCounter,
  appPage,
  approve,
  askConsent,
  launch,
  openSheet,
  person,
} from '../support/browser.mjs';
import { ctl, newAccount, runningVersion } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

const TELEMETRY = 'https://telemetry.example/';

test('a red update leads with keeping the current version, and a second approver applies it', async () => {
  const owner = newAccount('owner');
  const admin = newAccount('admin');
  const pkg = 'com.test.red';
  install(owner, { pkg, version: '1.0.0', tag: 'red1' });
  const { appId } = ctl(owner, [
    'instance',
    'create',
    '--name',
    'app',
    '--package',
    pkg,
    '--version',
    '1.0.0',
  ]);
  // Installed once the owner has its organization, so the organization's admin may use it.
  install(owner, { pkg, version: '2.0.0', tag: 'red1', extra: ['--outbound', TELEMETRY] });
  const org = ctl(owner, ['org', 'list']).namespaces[0].orgId;
  const { token } = ctl(owner, ['org', 'invite', org, '--role', 'admin'], { passkey: true });
  ctl(admin, ['org', 'join', token]);

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    const app = await appPage(me, { label: label(pkg) });

    // The red sheet: the reason, and the safe choice first.
    let made = await askConsent(app, { kind: 'upgrade', subject: { app: appId, toVersion: '2.0.0' } });
    let { model, text } = await openSheet(me, made, 'update-red');
    expect(model.update.class).toBe('red');
    expect(model.update.reasons[0]).toBe('This update can send data to a new place: telemetry.example.');
    expect(model.update.keepByDefault).toBe(true);
    expect(model.update.keepLabel).toBe('Keep 1.0.0');
    expect(model.update.approveLabel).toBe('Approve with passkey');
    expect(text).toContain('telemetry.example');
    await me.page.click('#decline');
    await me.page.waitForURL(/result=declined/);
    expect(runningVersion(owner, appId)).toBe('1.0.0');

    // The organization asks for a second approver, set on the page.
    made = await askConsent(app, {
      kind: 'updatePolicy',
      subject: { org, policy: { secondApprover: true } },
    });
    ({ model } = await openSheet(me, made, 'update-policy'));
    expect(JSON.stringify(model.policy.afterWords)).toContain('two owners or admins');
    await approve(me);

    // The owner's approval is the first of two and applies nothing.
    made = await askConsent(app, { kind: 'upgrade', subject: { app: appId, toVersion: '2.0.0' } });
    ({ model } = await openSheet(me, made, 'update-red-first-of-two'));
    expect(model.update.approveLabel).toBe('Approve (1 of 2)');
    await approve(me);
    expect(runningVersion(owner, appId)).toBe('1.0.0');
    const alone = ctl(owner, ['instance', 'update', appId, '--version', '2.0.0'], {
      passkey: true,
      allowFail: true,
    });
    expect(alone.failed).toBe(true);
    expect(runningVersion(owner, appId)).toBe('1.0.0');

    // The admin's updates page: locked until their passkey, then the update waits there.
    const second = await person(browser, admin);
    const adminApp = await appPage(second, { label: label(pkg) });
    made = await askConsent(adminApp, { kind: 'updates', subject: {} });
    ({ model, text } = await openSheet(second, made, 'updates-locked'));
    expect(model.locked).toBe(true);
    expect(model.updates).toBeUndefined();
    expect(text).toContain('Show my updates');
    await advanceCounter(second);
    await second.page.click('#approve');
    await second.page.waitForSelector('.hub-row');
    expect(await second.page.innerText('.hub-row')).toContain('Approved first by');

    // Its sheet is the second of two; approving it applies the update.
    await second.page.click('.hub-row button');
    await second.page.waitForURL(/\/consent\//);
    await second.page.waitForSelector('#content:not([hidden])');
    expect(await second.page.innerText('#approve-label')).toBe('Approve and update (2 of 2)');
    await advanceCounter(second);
    await second.page.click('#approve');
    await second.page.waitForURL(/\/updates\//);
    await expect.poll(() => runningVersion(owner, appId)).toBe('2.0.0');
  } finally {
    await browser.close();
  }
});
