/**
 * Generates /llms.txt — a machine-readable index of the docs for LLM/AI tools
 * (the emerging llms.txt convention). Built from the docs content collection so
 * it never drifts from the pages.
 */
import type { APIRoute } from 'astro';
import { getCollection } from 'astro:content';

const SITE = 'https://calimero-network.github.io';
const BASE = '/swift-sdk';

const TRACKS: Record<string, string> = {
  'get-started': 'Get Started — install, authenticate, and make your first call',
  understand: 'Understand — the SDK architecture, the actor/token model, and glossary',
  guides: 'Guides — contexts & apps, RPC, groups, blobs, and the SwiftUI frontend',
  reference: 'Reference — the Mero actor, the admin & auth API surface, and errors',
};

export const GET: APIRoute = async () => {
  const docs = await getCollection('docs');

  const url = (id: string) => {
    const slug = id.replace(/\.(md|mdx)$/, '').replace(/\/index$/, '');
    return `${SITE}${BASE}/${slug}/`.replace(/\/+$/, '/');
  };

  const byTrack: Record<string, typeof docs> = {};
  for (const entry of docs) {
    const track = entry.id.split('/')[0];
    if (!TRACKS[track]) continue;
    (byTrack[track] ??= []).push(entry);
  }

  const lines: string[] = [
    '# MeroKit — Calimero Swift SDK',
    '',
    '> The native Swift SDK for Calimero: a zero-dependency iOS/macOS client for a',
    '> remote Calimero node. async/await auth with reactive, single-use token',
    '> refresh (the Mero actor serialises it), JSON-RPC contract calls, the full',
    '> admin API, SSO deep-link login, Keychain token storage, and an optional',
    '> SwiftUI frontend (MeroKitUI). Built on URLSession/Foundation/Security.',
    '',
    `Docs site: ${SITE}${BASE}/`,
    '',
  ];

  for (const track of Object.keys(TRACKS)) {
    const entries = (byTrack[track] ?? []).sort(
      (a, b) => (a.data.sidebar?.order ?? 0) - (b.data.sidebar?.order ?? 0),
    );
    if (!entries.length) continue;
    lines.push(`## ${TRACKS[track]}`, '');
    for (const e of entries) {
      const desc = e.data.description ? `: ${e.data.description}` : '';
      lines.push(`- [${e.data.title}](${url(e.id)})${desc}`);
    }
    lines.push('');
  }

  return new Response(lines.join('\n'), {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
};
