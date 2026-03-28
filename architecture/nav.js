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
        <h2>Calimero <em>Core</em></h2>
        <p>Architecture Reference</p>
      </div>
      <div class="sidebar-search">
        <input type="text" id="nav-search" placeholder="Search pages..." autocomplete="off"/>
      </div>
      <div class="sidebar-nav" id="nav-links"></div>
      <div class="sidebar-footer">
        <a href="${REPO}" target="_blank" rel="noopener">GitHub &rarr;</a>
        &nbsp;&middot;&nbsp; v0.10.1-rc.8
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

    const search = sb.querySelector('#nav-search');
    search.addEventListener('input', () => {
      const q = search.value.toLowerCase();
      linksEl.querySelectorAll('.nav-link').forEach(a => {
        a.style.display = a.dataset.label.includes(q) ? '' : 'none';
      });
      linksEl.querySelectorAll('.nav-section').forEach(s => {
        const next = s.nextElementSibling;
        if (!next || next.classList.contains('nav-section')) { s.style.display = 'none'; return; }
        let hasVisible = false;
        let el = next;
        while (el && !el.classList.contains('nav-section')) {
          if (el.style.display !== 'none') hasVisible = true;
          el = el.nextElementSibling;
        }
        s.style.display = hasVisible ? '' : 'none';
      });
    });
  }

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

  document.addEventListener('DOMContentLoaded', () => {
    buildSidebar();
    tabSystem();
  });

  window.arch = { ghLink, buildBreadcrumb, REPO, PAGES_BASE };
})();
