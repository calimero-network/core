import { expect, test } from '@playwright/test';
import { appPage, approve, askConsent, launch, openSheet, person } from '../support/browser.mjs';
import { ctl, newAccount } from '../support/ctl.mjs';
import { install, label } from '../support/packages.mjs';

test('an app asks to act as the person in another app, for exactly the methods its release lists', async () => {
  const owner = newAccount('owner');
  const [calendar, booking] = ['com.test.calendar', 'com.test.booking'];
  const granted = ['recordWho', 'whoView'];
  install(owner, { pkg: calendar, version: '1.0.0', tag: 'cal' });
  install(owner, {
    pkg: booking,
    version: '1.0.0',
    tag: 'booking',
    patch: (m) => {
      m.uses = { cal: { package: calendar, range: '^1', mode: 'attach', methods: granted } };
    },
  });
  const source = ctl(owner, ['instance', 'create', '--name', 'booking', '--package', booking]).appId;
  const target = ctl(owner, ['instance', 'create', '--name', 'calendar', '--package', calendar]).appId;
  const relay = (key) =>
    ctl(owner, ['call', '-i', source, 'callWho', '--args', JSON.stringify({ app: target, key })], {
      allowFail: true,
    });
  const recorded = (key) =>
    JSON.stringify(ctl(owner, ['call', '-i', target, '--view', 'get', '--args', JSON.stringify({ key })]));

  relay('before');
  await expect.poll(() => recorded('before')).toContain(`app:${source}`);
  expect(recorded('before')).not.toContain(owner.id);

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    const app = await appPage(me, { label: label(booking) });
    const made = await askConsent(app, { kind: 'delegate', subject: { app: source, target } });
    const { model } = await openSheet(me, made, 'delegate');
    expect(model.title).toBe('Allow booking to act as you in calendar');
    expect(model.delegate.methods).toEqual(granted);
    await approve(me);
  } finally {
    await browser.close();
  }

  // Once granted, the calendar sees the person through the booking app.
  let n = 0;
  await expect
    .poll(() => {
      relay(`after${n}`);
      return recorded(`after${n++}`);
    })
    .toContain(`account:${owner.id} via:${source}`);
});
