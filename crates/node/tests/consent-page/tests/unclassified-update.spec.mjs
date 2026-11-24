import { expect, test } from '@playwright/test';
import { appPage, askConsent, launch, openSheet, person } from '../support/browser.mjs';
import { ctl, newAccount } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

const TELEMETRY = 'https://telemetry.example/';

// A release whose running version was uninstalled (allowed while another release ships the same
// code) cannot be classified: its update is red with the reason `unclassified`. The node says so to
// an unbound session, but answers the app's own session `404 no such app`, so the sheet is unreachable.
test("an update from an uninstalled release is red and unclassified on the app's own sheet", async () => {
  test.fail(
    true,
    "the app's own session gets 404 NOT_FOUND no such app for an app whose release was uninstalled",
  );
  const owner = newAccount('owner');
  const pkg = 'com.test.unclassified';
  install(owner, { pkg, version: '1.0.0', tag: 'uncl' });
  install(owner, { pkg, version: '2.0.0', tag: 'uncl', extra: ['--outbound', TELEMETRY] });
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
  ctl(owner, ['app', 'uninstall', pkg, '--version', '1.0.0']);

  const unbound = ctl(owner, ['raw', 'get', `apps/${appId}/update`]);
  expect(unbound.available.class).toBe('red');
  expect(unbound.available.reasons[0].code).toBe('unclassified');

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    const app = await appPage(me, { label: label(pkg) });
    const made = await askConsent(app, { kind: 'upgrade', subject: { app: appId, toVersion: '2.0.0' } });
    const { model } = await openSheet(me, made, 'update-unclassified');
    expect(model.update.class).toBe('red');
    expect(model.update.reasons.length).toBeGreaterThan(0);
  } finally {
    await browser.close();
  }
});
