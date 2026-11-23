import { expect, test } from '@playwright/test';
import { appPage, approve, askConsent, launch, openSheet, person } from '../support/browser.mjs';
import { ctl, newAccount, runningVersion } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

// The hold is at least 24 hours and a test has no clock to move, so this checks the waiting state:
// the policy is stored, the update is offered and not applied, and its sheet says it is on hold.
test('under an automatic policy a green update waits for its hold', async () => {
  const owner = newAccount('owner');
  const pkg = 'com.test.hold';
  install(owner, { pkg, version: '1.0.0', tag: 'hold1' });
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
    const policy = { autoApply: 'noDestinations', holdHours: 24, secondApprover: false };
    let made = await askConsent(app, { kind: 'updatePolicy', subject: { app: appId, policy } });
    let { model } = await openSheet(me, made, 'policy-auto-apply');
    expect(model.policy.afterWords[0]).toContain('applies on its own after a 24 h hold');
    await approve(me);

    install(owner, { pkg, version: '1.1.0', tag: 'hold2' });
    await expect
      .poll(() => ctl(owner, ['raw', 'get', `apps/${appId}/update`]).available?.version)
      .toBe('1.1.0');
    const update = ctl(owner, ['raw', 'get', `apps/${appId}/update`]);
    expect(update.available.class).toBe('green');
    expect(update.policy.autoApply).toBe('noDestinations');
    expect(runningVersion(owner, appId)).toBe('1.0.0');

    made = await askConsent(app, { kind: 'upgrade', subject: { app: appId, toVersion: '1.1.0' } });
    ({ model } = await openSheet(me, made, 'update-green-hold'));
    expect(model.update.hold).toContain('apply on its own after the 24 h hold');
  } finally {
    await browser.close();
  }
});
