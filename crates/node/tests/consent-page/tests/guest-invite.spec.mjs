import { expect, test } from '@playwright/test';
import {
  appPage,
  approve,
  askConsent,
  launch,
  nodeClient,
  openSheet,
  person,
  signedCall,
} from '../support/browser.mjs';
import { ctl, newAccount } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

test('an outsider accepts an instance invite and reaches that instance only, until the org turns guests off', async () => {
  const owner = newAccount('owner');
  const guest = newAccount('guest');
  const pkg = 'com.test.guests';
  install(owner, { pkg, version: '1.0.0', tag: 'guests', extra: ['--guests', 'allowed'] });
  const shared = ctl(owner, ['instance', 'create', '--name', 'shared', '--package', pkg]).appId;
  const private_ = ctl(owner, ['instance', 'create', '--name', 'private', '--package', pkg]).appId;
  const org = ctl(owner, ['instance', 'get', shared]).org;
  const { token } = ctl(owner, ['raw', 'post', `apps/${shared}/invites`, '{}'], { passkey: true });
  const read = (app) =>
    ctl(guest, ['call', '-i', app, '--view', 'get', '--args', '{"key":"k"}'], { allowFail: true });

  const browser = await launch();
  try {
    const them = await person(browser, guest);
    const app = await appPage(them, { label: label(pkg) });
    const made = await askConsent(app, { kind: 'acceptInvite', subject: { token } });
    const { text } = await openSheet(them, made, 'guest-invite');
    expect(text).toContain('as a guest: this app only, not the organization');
    expect(await approve(them)).toContain('result=approved');

    await expect.poll(() => ctl(guest, ['instance', 'list']).apps.map((a) => a.appId)).toEqual([shared]);
    expect(read(shared).failed).toBeUndefined();
    expect(read(private_).failed).toBe(true);

    const admin = await person(browser, owner);
    await nodeClient(admin);
    await signedCall(
      admin,
      (o) => window.n.orgs.settings({ org: o, allowGuests: false }, window.confirm1()),
      org,
    );
    await expect.poll(() => read(shared).failed).toBe(true);
  } finally {
    await browser.close();
  }
});
