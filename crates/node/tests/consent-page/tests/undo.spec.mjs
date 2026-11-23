import { expect, test } from '@playwright/test';
import { ctl, newAccount, runningVersion } from '../support/ctl.mjs';
import { install } from '../support/packages.mjs';

// The consent page has no undo sheet: an undo is the owner's passkey on `POST /apps/{app}/undo`
// (`meroctl instance undo`). It is kept here with the page's other update flows.
test('an undo says what it loses, then returns the old release with its old data', async () => {
  const owner = newAccount('owner');
  const pkg = 'com.test.undo';
  install(owner, { pkg, version: '1.0.0', tag: 'undo1' });
  install(owner, { pkg, version: '1.1.0', tag: 'undo2' });
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
  const call = (method, args, view = false) =>
    ctl(owner, ['call', '-i', appId, method, ...(view ? ['--view'] : []), '--args', JSON.stringify(args)]);

  call('set', { key: 'before', value: 'old' });
  ctl(owner, ['instance', 'update', appId, '--version', '1.1.0'], { passkey: true });
  await expect.poll(() => runningVersion(owner, appId)).toBe('1.1.0');
  call('set', { key: 'after', value: 'new' });

  const plan = ctl(owner, ['instance', 'undo', appId]);
  expect(plan.available).toBe(true);
  expect(plan.apps[0].to.version).toBe('1.0.0');
  expect(plan.apps[0].lostWrites.any).toBe(true);
  expect(plan.apps[0].lostWrites.entriesSince).toBeGreaterThan(0);

  ctl(owner, ['instance', 'undo', appId, '--yes'], { passkey: true });
  await expect.poll(() => runningVersion(owner, appId), { timeout: 60_000 }).toBe('1.0.0');
  await expect.poll(() => JSON.stringify(call('get', { key: 'before' }, true))).toContain('old');
  expect(JSON.stringify(call('get', { key: 'after' }, true))).not.toContain('new');
});
