import { expect, test } from '@playwright/test';
import { appPage, approve, askConsent, launch, openSheet, person, shoot } from '../support/browser.mjs';
import { ctl, newAccount } from '../support/ctl.mjs';
import { label, pack } from '../support/packages.mjs';

/** A person in Acme and Globex, `pkg@1.0.0` installed for `orgs` (names), and its install sheet open. */
async function sheetFor(browser, pkg, orgs) {
  const owner = newAccount('pair');
  const ids = {
    Acme: ctl(owner, ['org', 'create', 'Acme']).orgId,
    Globex: ctl(owner, ['org', 'create', 'Globex']).orgId,
  };
  const mpk = pack({ pkg, version: '1.0.0', tag: pkg.split('.').pop() });
  for (const o of orgs) ctl(owner, ['app', 'install', '--path', mpk, '--org', ids[o]]);
  const me = await person(browser, owner);
  const app = await appPage(me, { label: label(pkg) });
  const made = await askConsent(app, {
    kind: 'install',
    subject: { package: pkg, version: '1.0.0', name: 'notes' },
  });
  const { text } = await openSheet(me, made);
  const createdIn = () => ctl(owner, ['instance', 'list']).apps.find((a) => a.name === 'notes').org;
  return { me, ids, text, createdIn };
}

test("a release private to one of the person's organizations is created there with no choice", async () => {
  const browser = await launch();
  try {
    const { me, ids, text, createdIn } = await sheetFor(browser, 'com.test.acmeonly', ['Acme']);
    await shoot(me, 'install-private-filled');
    expect(text).toContain('Private to Acme');
    expect(text).toContain('notes is created for you in Acme');
    expect(await me.page.locator('#org').count()).toBe(0);
    expect(await approve(me)).toContain('result=approved');
    expect(createdIn()).toBe(ids.Acme);
  } finally {
    await browser.close();
  }
});

test("a release private to both of the person's organizations asks which one, and the sheet names it", async () => {
  const browser = await launch();
  try {
    const { me, ids, text, createdIn } = await sheetFor(browser, 'com.test.bothorgs', ['Acme', 'Globex']);
    await shoot(me, 'install-two-orgs');
    expect(text).toContain('Private to Acme and Globex');
    expect(text).toMatch(/create it in/i);
    await expect(me.page.locator('#approve')).toBeDisabled();

    await me.page.selectOption('#org', ids.Globex);
    await expect(me.page.locator('#created')).toHaveText(
      'notes is created for you in Globex: bothorgs 1.0.0.',
    );
    await shoot(me, 'install-two-orgs-chosen');
    expect(await approve(me)).toContain('result=approved');
    expect(createdIn()).toBe(ids.Globex);
  } finally {
    await browser.close();
  }
});
