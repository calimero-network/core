import { expect, test } from '@playwright/test';
import { appPage, approve, askConsent, launch, openSheet, person, shoot } from '../support/browser.mjs';
import { ctl, newAccount } from '../support/ctl.mjs';
import { label, pack } from '../support/packages.mjs';

test('a person in two organizations chooses the one an install is for, and sees the release is private to it', async () => {
  const owner = newAccount('pair');
  const acme = ctl(owner, ['org', 'create', 'Acme']).orgId;
  ctl(owner, ['org', 'create', 'Globex']);
  const pkg = 'com.test.private';
  ctl(owner, ['app', 'install', '--path', pack({ pkg, version: '1.0.0', tag: 'private' }), '--org', acme]);

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    const app = await appPage(me, { label: label(pkg) });
    const made = await askConsent(app, {
      kind: 'install',
      subject: { package: pkg, version: '1.0.0', name: 'notes' },
    });
    const { text } = await openSheet(me, made);
    await shoot(me, 'install-two-orgs');
    expect(text).toContain('Private to Acme');
    expect(text).toMatch(/create it in/i);
    await expect(me.page.locator('#approve')).toBeDisabled();

    await me.page.selectOption('#org', acme);
    await shoot(me, 'install-two-orgs-chosen');
    expect(await approve(me)).toContain('result=approved');
    const created = ctl(owner, ['instance', 'list']).apps.find((a) => a.name === 'notes');
    expect(created.org).toBe(acme);
  } finally {
    await browser.close();
  }
});
