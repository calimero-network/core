import React, { useState } from 'react';
import {
  ConnectButton,
  MeroProvider,
  useApps,
  useExecute,
  useMero,
  useSubscription,
  useView,
} from '@calimero-network/mero-react';

/**
 * A KV store on Calimero 1.0.
 *
 * Served by the node (`{label}.{apps-domain}`), the page is its own node:
 * `servedByNode`. Run against `merod local` in development with
 * `VITE_NODE_URL=http://127.0.0.1:2501` (mock attestation, dev only), or
 * against a Calimero Cloud cluster by name with `VITE_CLUSTER=eu-shared-1`.
 */
const DEV_NODE = import.meta.env.VITE_NODE_URL as string | undefined;
const CLUSTER = import.meta.env.VITE_CLUSTER as string | undefined;
const MIN_RELEASE = (import.meta.env.VITE_MIN_RELEASE as string | undefined) ?? '2.3.109';

export default function App() {
  if (DEV_NODE) {
    return (
      <MeroProvider nodeUrl={DEV_NODE} allowMock>
        <Main />
      </MeroProvider>
    );
  }
  return CLUSTER ? (
    <MeroProvider cluster={CLUSTER} minReleaseVersion={MIN_RELEASE}>
      <Main />
    </MeroProvider>
  ) : (
    <MeroProvider servedByNode minReleaseVersion={MIN_RELEASE}>
      <Main />
    </MeroProvider>
  );
}

function Main() {
  const { isAuthenticated, appId, setAppId } = useMero();
  return (
    <main style={{ maxWidth: 640, margin: '48px auto', padding: '0 16px', fontFamily: 'system-ui' }}>
      <header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
        <h1>KV store</h1>
        <ConnectButton appName="KV store" />
      </header>
      {isAuthenticated && !appId && <PickApp onPick={setAppId} />}
      {isAuthenticated && appId && <Store />}
    </main>
  );
}

function PickApp({ onPick }: { onPick: (id: string) => void }) {
  const apps = useApps({ package: 'com.calimero.kv-store' });
  if (apps.loading) return <p>Loading apps…</p>;
  if (apps.failure) return <p role="alert">{apps.failure.message}</p>;
  if (apps.data.length === 0) return <p>No KV store instance yet. Create one with meroctl or the console.</p>;
  return (
    <ul>
      {apps.data.map((app) => (
        <li key={app.appId}>
          <button onClick={() => onPick(app.appId)}>{app.name || app.appId}</button>
        </li>
      ))}
    </ul>
  );
}

function Store() {
  const entries = useView<Record<string, string>>('entries');
  const { execute, loading, failure } = useExecute();
  const [key, setKey] = useState('');
  const [value, setValue] = useState('');

  useSubscription(undefined, () => void entries.refetch(), { onOpen: () => void entries.refetch() });

  return (
    <section>
      <form
        onSubmit={async (e) => {
          e.preventDefault();
          if (await execute('set', { key, value })) {
            setKey('');
            setValue('');
          }
          void entries.refetch();
        }}
      >
        <input placeholder="key" value={key} onChange={(e) => setKey(e.target.value)} />
        <input placeholder="value" value={value} onChange={(e) => setValue(e.target.value)} />
        <button disabled={loading || !key}>Set</button>
      </form>
      {failure && <p role="alert">{failure.message}</p>}
      <table>
        <tbody>
          {Object.entries(entries.data ?? {}).map(([k, v]) => (
            <tr key={k}>
              <td>{k}</td>
              <td>{v}</td>
              <td>
                <button onClick={() => void execute('remove', { key: k }).then(() => entries.refetch())}>Remove</button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
