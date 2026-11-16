import { defineMiddleware } from 'astro:middleware';

/**
 * Base-prefix in-content links on every HTML response — in dev AND in the build.
 *
 * Astro does not add the site `base` to in-content links (markdown links or
 * component `href`s like Starlight's LinkCard/Card/hero); only its own nav gets
 * it. So authors write base-less links (`/guides/x/`) and this rewrites them
 * to `/kotlin-sdk/guides/x/` at request time (dev) and prerender time (build).
 *
 * Skips links already under the base and protocol-relative (`//`) URLs.
 */
const BASE = '/kotlin-sdk'; // keep in sync with astro.config.mjs
const re = new RegExp(`href="/(?!${BASE.slice(1)}/)(?!/)`, 'g');

export const onRequest = defineMiddleware(async (_ctx, next) => {
  const res = await next();
  const type = res.headers.get('content-type') ?? '';
  if (!type.includes('text/html')) return res;

  const html = (await res.text()).replace(re, `href="${BASE}/`);
  const headers = new Headers(res.headers);
  headers.delete('content-length'); // body length changed
  return new Response(html, { status: res.status, statusText: res.statusText, headers });
});
