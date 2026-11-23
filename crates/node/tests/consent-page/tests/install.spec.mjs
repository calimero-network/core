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
import { ctl, newAccount } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

test('the install sheet says what gets created and where data can go, and approving creates the instance', async () => {
  const owner = newAccount('owner');
  const pkg = 'com.test.install';
  const near = 'https://inference.near.ai/v1/';
  install(owner, { pkg, version: '1.0.0', tag: 'install', extra: ['--outbound', near] });

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    const app = await appPage(me, { label: label(pkg) });
    const made = await askConsent(app, {
      kind: 'install',
      subject: { package: pkg, version: '1.0.0', name: 'notes' },
    });
    const { model, text } = await openSheet(me, made, 'install');

    expect(model.kind).toBe('install');
    expect(model.title).toBe('Start using install');
    expect(model.install.destinations).toEqual(['inference.near.ai']);
    expect(model.install.where).toBe('This app can send data to: inference.near.ai.');
    expect(model.install.updates[0]).toBe('You approve each update. Nothing changes until you do.');
    expect(text).toContain('Create with passkey');
    expect(text).toContain('inference.near.ai');

    expect(await approve(me)).toContain('result=approved');
    const listed = ctl(owner, ['instance', 'list']).apps.map((a) => a.name);
    expect(listed).toContain('notes');
  } finally {
    await browser.close();
  }
});
