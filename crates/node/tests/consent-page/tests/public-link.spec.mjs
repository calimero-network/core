import { expect, test } from '@playwright/test';
import { launch, nodeClient, person, signedCall } from '../support/browser.mjs';
import { ctl, newAccount } from '../support/ctl.mjs';
import { ports } from '../support/env.mjs';
import { install } from '../support/packages.mjs';

// Public links have no consent sheet: the owner's passkey goes through the API.
test('a visitor calls a public method until the link is revoked or its rate is passed', async () => {
  const owner = newAccount('owner');
  const pkg = 'com.test.links';
  install(owner, { pkg, version: '1.0.0', tag: 'links' });
  const app = ctl(owner, ['instance', 'create', '--name', 'forms', '--package', pkg]).appId;

  const browser = await launch();
  try {
    const me = await person(browser, owner);
    await nodeClient(me);
    const made = await signedCall(
      me,
      (a) => window.n.apps.publicLinkCreate({ app: a, label: 'Survey', ratePerMinute: 3 }, window.confirm1()),
      app,
    );
    expect(made.token).toBeTruthy();

    const visit = (method, args) =>
      me.page.evaluate(
        async ({ app, token, method, args, port }) => {
          const v = new Mero.CalimeroClient({
            baseUrl: `http://localhost:${port}`,
            policy: { allowMock: true },
          });
          await v.useVisitor({ link: token, seed: new Uint8Array(32).fill(7) });
          try {
            return { ok: await v.apps.call({ app, method, args }) };
          } catch (e) {
            return { error: `${e.name} ${e.message}` };
          }
        },
        { app, token: made.token, method, args, port: ports()[0] },
      );

    expect((await visit('signGuestbook', { note: 'hello' })).ok).toBeTruthy();
    expect((await visit('set', { key: 'k', value: 'v' })).error).toContain(
      "calls only the app's public methods",
    );

    const answers = [];
    for (let i = 0; i < 6; i++) answers.push(await visit('signGuestbook', { note: `n${i}` }));
    expect(answers.some((a) => /429|TOO_MANY/.test(a.error ?? ''))).toBe(true);

    await signedCall(me, (x) => window.n.apps.publicLinkRevoke(x, window.confirm1()), {
      app,
      link: made.link,
    });
    await expect
      .poll(async () => (await visit('signGuestbook', { note: 'late' })).error)
      .toContain('revoked');
  } finally {
    await browser.close();
  }
});
