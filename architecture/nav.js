/* Calimero Core Architecture — Shared Navigation */
(function () {
  'use strict';

  const REPO = 'https://github.com/calimero-network/core';
  const PAGES_BASE = getBase();

  function getBase() {
    const p = location.pathname;
    if (p.includes('/crates/')) return '../';
    return './';
  }

  const NAV = [
    { label: 'Home', href: 'index.html', dot: '#f59e0b' },
    { section: 'For Builders' },
    { label: 'Getting Started', href: 'getting-started.html', dot: '#10b981' },
    { label: 'Core Concepts', href: 'concepts.html', dot: '#10b981' },
    { label: 'SDK Reference', href: 'crates/sdk.html', dot: '#f97316' },
    { label: 'App Lifecycle', href: 'app-lifecycle.html', dot: '#06b6d4' },
    { label: 'Example: Chat', href: 'example-chat.html', dot: '#10b981' },
    { label: 'Example: Docs', href: 'example-docs.html', dot: '#3b82f6' },
    { section: 'For Operators' },
    { label: 'merod & meroctl', href: 'crates/tools.html', dot: '#f59e0b' },
    { label: 'Config Reference', href: 'config-reference.html', dot: '#f97316' },
    { label: 'TEE Mode', href: 'tee-mode.html', dot: '#ec4899' },
    { label: 'Release Process', href: 'release.html', dot: '#10b981' },
    { label: 'Auth Service', href: 'crates/auth.html', dot: '#84cc16' },
    { section: 'Architecture Deep-Dive' },
    { label: 'System Overview', href: 'system-overview.html', dot: '#3b82f6' },
    { label: 'Local Governance', href: 'local-governance.html', dot: '#10b981' },
    { label: 'Sequence Diagrams', href: 'sequence-diagrams.html', dot: '#ec4899' },
    { label: 'Wire Protocol', href: 'wire-protocol.html', dot: '#8b5cf6' },
    { label: 'Storage Schema', href: 'storage-schema.html', dot: '#06b6d4' },
    { label: 'Error Flows', href: 'error-flows.html', dot: '#ef4444' },
    { label: 'Metrics Reference', href: 'metrics-reference.html', dot: '#10b981' },
    { label: 'Dependency Explorer', href: 'dependency-explorer.html', dot: '#f59e0b' },
    { label: 'Glossary', href: 'glossary.html', dot: '#d4d4dc' },
    { section: 'Crate Internals' },
    { label: 'Node', href: 'crates/node.html', dot: '#3b82f6', sub: true },
    { label: 'Context & Groups', href: 'crates/context.html', dot: '#10b981', sub: true },
    { label: 'Network & P2P', href: 'crates/network.html', dot: '#8b5cf6', sub: true },
    { label: 'Storage', href: 'crates/store.html', dot: '#06b6d4', sub: true },
    { label: 'Sync Engine', href: 'crates/sync.html', dot: '#f59e0b', sub: true },
    { label: 'WASM Runtime', href: 'crates/runtime.html', dot: '#ec4899', sub: true },
    { label: 'Server & API', href: 'crates/server.html', dot: '#f97316', sub: true },
    { label: 'Causal DAG', href: 'crates/dag.html', dot: '#f59e0b', sub: true },
  ];

  /* ── Full-text search index ── */

  let searchIndex = null;
  let searchReady = false;
  let selectedIdx = -1;

  function buildIndex() {
    const pages = NAV.filter(n => n.href);
    return Promise.all(pages.map(item => {
      const url = PAGES_BASE + item.href;
      return fetch(url).then(r => r.ok ? r.text() : '').then(html => {
        const doc = new DOMParser().parseFromString(html, 'text/html');
        doc.querySelectorAll('script, style, nav, .sidebar, .breadcrumb').forEach(el => el.remove());
        const headings = Array.from(doc.querySelectorAll('h1, h2, h3, h4')).map(h => h.textContent.trim());
        const text = (doc.body ? doc.body.textContent : '').replace(/\s+/g, ' ').trim();
        return { title: item.label, href: item.href, headings, text };
      }).catch(() => ({ title: item.label, href: item.href, headings: [], text: '' }));
    }));
  }

  function searchDocs(query, index) {
    const q = query.toLowerCase().trim();
    if (!q) return [];
    const tokens = q.split(/\s+/).filter(Boolean);
    const scored = [];
    for (const page of index) {
      let score = 0;
      const titleLow = page.title.toLowerCase();
      const headingsLow = page.headings.join(' ').toLowerCase();
      const textLow = page.text.toLowerCase();
      for (const t of tokens) {
        if (titleLow.includes(t)) score += 10;
        if (headingsLow.includes(t)) score += 5;
        if (textLow.includes(t)) score += 1;
      }
      if (score > 0) scored.push({ ...page, score });
    }
    scored.sort((a, b) => b.score - a.score);
    return scored.slice(0, 12);
  }

  function getExcerpt(text, query) {
    const tokens = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
    const lower = text.toLowerCase();
    let best = -1;
    for (const t of tokens) {
      const idx = lower.indexOf(t);
      if (idx !== -1) { best = idx; break; }
    }
    if (best === -1) return '';
    const start = Math.max(0, best - 60);
    const end = Math.min(text.length, best + 140);
    let slice = (start > 0 ? '...' : '') + text.slice(start, end).trim() + (end < text.length ? '...' : '');
    for (const t of tokens) {
      const re = new RegExp('(' + t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')', 'gi');
      slice = slice.replace(re, '<mark>$1</mark>');
    }
    return slice;
  }

  /* ── Search overlay DOM ── */

  function createSearchOverlay() {
    const overlay = document.createElement('div');
    overlay.className = 'search-overlay';
    overlay.id = 'search-overlay';
    overlay.innerHTML = `
      <div class="search-modal">
        <div class="search-input-row">
          <svg class="search-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
          <input id="search-input" type="text" placeholder="Search documentation..." autocomplete="off" spellcheck="false"/>
          <span class="search-esc-hint">ESC</span>
        </div>
        <div class="search-results" id="search-results">
          <div class="search-hint">Type to search across all pages.<br><strong>Tip:</strong> Use <kbd style="font-family:'JetBrains Mono',monospace;font-size:.7rem;padding:1px 4px;border-radius:3px;border:1px solid var(--border-hi);background:var(--surface2)">⌘K</kbd> to open search anytime.</div>
        </div>
        <div class="search-footer-bar">
          <span><kbd>↑↓</kbd> navigate</span>
          <span><kbd>↵</kbd> open</span>
          <span><kbd>esc</kbd> close</span>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) closeSearch();
    });

    const input = overlay.querySelector('#search-input');
    let debounce = null;
    input.addEventListener('input', () => {
      clearTimeout(debounce);
      debounce = setTimeout(() => runSearch(input.value), 150);
    });

    input.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowDown') { e.preventDefault(); moveSelection(1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); moveSelection(-1); }
      else if (e.key === 'Enter') {
        e.preventDefault();
        const items = document.querySelectorAll('.search-result-item');
        if (items[selectedIdx]) items[selectedIdx].click();
      }
      else if (e.key === 'Escape') { closeSearch(); }
    });
  }

  function runSearch(query) {
    const results = document.getElementById('search-results');
    if (!results) return;
    selectedIdx = -1;
    if (!query.trim() || query.trim().length < 2) {
      results.innerHTML = '<div class="search-hint">Type to search across all pages.<br><strong>Tip:</strong> Use <kbd style="font-family:\'JetBrains Mono\',monospace;font-size:.7rem;padding:1px 4px;border-radius:3px;border:1px solid var(--border-hi);background:var(--surface2)">⌘K</kbd> to open search anytime.</div>';
      return;
    }
    if (!searchReady) {
      results.innerHTML = '<div class="search-hint">Building search index...</div>';
      return;
    }
    const matches = searchDocs(query, searchIndex);
    if (matches.length === 0) {
      results.innerHTML = '<div class="search-hint">No results for <strong>' + escHtml(query) + '</strong></div>';
      return;
    }
    results.innerHTML = matches.map((m, i) => {
      const titleHtml = highlightTokens(m.title, query);
      const excerpt = getExcerpt(m.text, query);
      return '<a class="search-result-item' + (i === 0 ? ' selected' : '') + '" href="' + PAGES_BASE + m.href + '">' +
        '<div class="search-result-header"><span class="search-result-title">' + titleHtml + '</span></div>' +
        (excerpt ? '<p class="search-result-excerpt">' + excerpt + '</p>' : '') +
        '</a>';
    }).join('');
    selectedIdx = 0;
  }

  function highlightTokens(text, query) {
    const tokens = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
    let out = escHtml(text);
    for (const t of tokens) {
      const re = new RegExp('(' + t.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')', 'gi');
      out = out.replace(re, '<mark>$1</mark>');
    }
    return out;
  }

  function escHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function moveSelection(dir) {
    const items = document.querySelectorAll('.search-result-item');
    if (!items.length) return;
    items.forEach(i => i.classList.remove('selected'));
    selectedIdx = (selectedIdx + dir + items.length) % items.length;
    items[selectedIdx].classList.add('selected');
    items[selectedIdx].scrollIntoView({ block: 'nearest' });
  }

  function openSearch() {
    const overlay = document.getElementById('search-overlay');
    if (!overlay) return;
    overlay.classList.add('open');
    const input = overlay.querySelector('#search-input');
    input.value = '';
    input.focus();
    runSearch('');
    if (!searchReady && !searchIndex) {
      buildIndex().then(idx => { searchIndex = idx; searchReady = true; });
    }
  }

  function closeSearch() {
    const overlay = document.getElementById('search-overlay');
    if (overlay) overlay.classList.remove('open');
  }

  /* ── Sidebar builder ── */

  function currentPage() {
    const p = location.pathname;
    for (const item of NAV) {
      if (!item.href) continue;
      if (p.endsWith(item.href) || p.endsWith('/' + item.href)) return item.href;
    }
    if (p.endsWith('/') || p.endsWith('/architecture/') || p.endsWith('/architecture')) return 'index.html';
    return '';
  }

  function buildSidebar() {
    const sb = document.createElement('nav');
    sb.className = 'sidebar';
    sb.id = 'sidebar';

    const cur = currentPage();

    sb.innerHTML = `
      <div class="sidebar-logo">
        <div class="sidebar-logo-inner">
          <div class="sidebar-logo-text">
            <strong>Calimero <em>Core</em></strong>
            <span>Architecture Reference</span>
          </div>
        </div>
      </div>
      <div class="sidebar-search">
        <button id="open-search" class="docs-search-btn">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
          <span>Search docs\u2026</span>
          <kbd>\u2318K</kbd>
        </button>
      </div>
      <div class="sidebar-nav" id="nav-links"></div>
      <div class="sidebar-footer">
        <div class="sidebar-footer-brand">&copy; Calimero Network</div>
        <div class="sidebar-footer-links">
          <a href="${REPO}" target="_blank" rel="noopener">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/></svg>
            GitHub
          </a>
          <a href="https://calimero.network" target="_blank" rel="noopener">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="2" y1="12" x2="22" y2="12"/><path d="M12 2a15.3 15.3 0 014 10 15.3 15.3 0 01-4 10 15.3 15.3 0 01-4-10 15.3 15.3 0 014-10z"/></svg>
            Website
          </a>
        </div>
      </div>
    `;

    const themeBtn = document.createElement('button');
    themeBtn.id = 'theme-toggle';
    themeBtn.className = 'theme-toggle';
    themeBtn.textContent = '\u263e';
    themeBtn.title = 'Toggle light/dark mode';
    themeBtn.onclick = () => {
      const isLight = document.documentElement.classList.toggle('light');
      themeBtn.textContent = isLight ? '\u2600' : '\u263e';
      try { localStorage.setItem('arch-theme', isLight ? 'light' : 'dark'); } catch(e) {}
    };
    sb.querySelector('.sidebar-logo').appendChild(themeBtn);

    try {
      if (localStorage.getItem('arch-theme') === 'light') {
        document.documentElement.classList.add('light');
        themeBtn.textContent = '\u2600';
      }
    } catch(e) {}

    const linksEl = sb.querySelector('#nav-links');
    for (const item of NAV) {
      if (item.section) {
        const s = document.createElement('div');
        s.className = 'nav-section';
        s.textContent = item.section;
        linksEl.appendChild(s);
        continue;
      }
      const a = document.createElement('a');
      a.className = 'nav-link' + (item.sub ? ' sub' : '') + (item.href === cur ? ' active' : '');
      a.href = PAGES_BASE + item.href;
      a.innerHTML = `<span class="nav-dot" style="background:${item.dot}"></span>${item.label}`;
      a.dataset.label = item.label.toLowerCase();
      linksEl.appendChild(a);
    }

    document.body.prepend(sb);

    const btn = document.createElement('button');
    btn.className = 'menu-toggle';
    btn.textContent = '\u2630';
    btn.onclick = () => sb.classList.toggle('open');
    document.body.prepend(btn);

    sb.querySelector('#open-search').addEventListener('click', openSearch);
  }

  /* ── Breadcrumb, tabs, ghLink ── */

  function buildBreadcrumb(items) {
    const bc = document.querySelector('.breadcrumb');
    if (!bc) return;
    bc.innerHTML = items.map((item, i) => {
      if (i === items.length - 1) return `<span>${item.label}</span>`;
      return `<a href="${item.href}">${item.label}</a><span class="sep">/</span>`;
    }).join('');
  }

  function tabSystem() {
    document.querySelectorAll('[data-tabs]').forEach(container => {
      const tabs = container.querySelectorAll('.tab');
      const panels = container.parentElement.querySelectorAll('.panel');
      tabs.forEach(tab => {
        tab.addEventListener('click', () => {
          tabs.forEach(t => t.classList.remove('on'));
          panels.forEach(p => p.classList.remove('on'));
          tab.classList.add('on');
          const target = document.getElementById(tab.dataset.target);
          if (target) target.classList.add('on');
        });
      });
    });
  }

  function ghLink(path, line) {
    const base = REPO + '/blob/master/';
    const url = line ? base + path + '#L' + line : base + path;
    return `<a class="gh-link" href="${url}" target="_blank" rel="noopener">${path}</a>`;
  }

  /* ── Keyboard shortcuts ── */

  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
      e.preventDefault();
      openSearch();
    }
    if (e.key === 'Escape') {
      closeSearch();
    }
  });

  /* ── Init ── */

  document.addEventListener('DOMContentLoaded', () => {
    buildSidebar();
    createSearchOverlay();
    tabSystem();
    buildIndex().then(idx => { searchIndex = idx; searchReady = true; });
  });

  window.arch = { ghLink, buildBreadcrumb, openSearch, REPO, PAGES_BASE };
})();
