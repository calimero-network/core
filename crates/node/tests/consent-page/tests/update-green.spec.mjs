import { expect, test } from '@playwright/test';
import { appPage, approve, askConsent, launch, openSheet, person } from '../support/browser.mjs';
import { ctl, newAccount, runningVersion } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

test('a green update says so in plain words, and approving moves the instance', async () => {
  const owner = newAccount('owner');
  const pkg = 'com.test.green';
  install(owner, { pkg, version: '1.0.0', tag: 'green1' });
  install(owner, { pkg, version: '1.1.0', tag: 'green2' });
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

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    const app = await appPage(me, { label: label(pkg) });
    const made = await askConsent(app, { kind: 'upgrade', subject: { app: appId, toVersion: '1.1.0' } });
    const { model, text } = await openSheet(me, made, 'update-green');
    const u = model.update;

    expect(u.class).toBe('green');
    expect(u.summary).toBe('Nothing changes where your data goes');
    expect(u.destinations.words).toBe('Unchanged: your data stays in Calimero');
    expect(u.approveLabel).toBe('Update with passkey');
    expect(u.keepLabel).toBe('Not now');
    expect(text.toLowerCase()).toContain('green');

    expect(await approve(me)).toContain('result=approved');
    await expect.poll(() => runningVersion(owner, appId)).toBe('1.1.0');
  } finally {
    await browser.close();
  }
});
