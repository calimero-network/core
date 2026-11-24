import { expect, test } from '@playwright/test';
import { appPage, approve, askConsent, launch, openSheet, person } from '../support/browser.mjs';
import { ctl, newAccount, runningVersion } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

test("a customer accepts its deployment's offer without naming a version, and no other version", async () => {
  const developer = newAccount('developer');
  const customer = newAccount('customer');
  const pkg = 'com.test.deployed';
  for (const [version, tag] of [
    ['1.0.0', 'dep1'],
    ['1.5.0', 'dep15'],
    ['2.0.0', 'dep2'],
  ]) {
    install(developer, { pkg, version, tag });
  }
  const { id } = ctl(developer, ['deployment', 'create', 'acme', '--owner', 'account'], { passkey: true });
  ctl(developer, ['deployment', 'offer', id, 'notes', '1.0.0', '--package', pkg], { passkey: true });
  const org = ctl(customer, ['org', 'create', 'Globex']).orgId;
  const body = JSON.stringify({ name: 'notes', deployment: id, package: pkg, org });
  const { appId } = ctl(customer, ['raw', 'post', 'apps', body]);
  ctl(developer, ['deployment', 'offer', id, 'notes', '2.0.0', '--package', pkg], { passkey: true });

  const browser = await launch();
  try {
    const me = await person(browser, customer);
    const app = await appPage(me, { label: label(pkg) });

    const other = askConsent(app, { kind: 'upgrade', subject: { app: appId, toVersion: '1.5.0' } });
    await expect(other).rejects.toThrow(/409|OUTSIDE_DEPLOYMENT/);

    const made = await askConsent(app, { kind: 'upgrade', subject: { app: appId } });
    const { model } = await openSheet(me, made, 'deployment-offer');
    expect(model.upgrade.to.version).toBe('2.0.0');
    expect(await approve(me)).toContain('result=approved');
    await expect.poll(() => runningVersion(customer, appId)).toBe('2.0.0');
  } finally {
    await browser.close();
  }
});
