/* React SDK Bindings Docs — Shared Navigation + Full-text Search */
(function () {
  'use strict';

  const REPO = 'https://github.com/calimero-network/mero-react';
  const PAGES_BASE = './';

  const NAV = [
    { section: 'Overview' },
    { label: 'Home',                 href: 'index.html',      dot: '#e3b341' },
    { label: 'Migrating from 10.x',  href: 'migration.html',  dot: '#a5ff11' },
    { section: 'Reference' },
    { label: 'MeroProvider',         href: 'provider.html',   dot: '#58a6ff' },
    { label: 'Hooks',                href: 'hooks.html',      dot: '#f778ba' },
    { label: 'Errors',               href: 'errors.html',     dot: '#ff7b72' },
    { label: 'Components & storage', href: 'components.html', dot: '#39d0d8' },
  ];

  function currentPage() {
    const p = location.pathname;
    for (const item of NAV) {
      if (!item.href) continue;
      if (p.endsWith(item.href) || p.endsWith('/' + item.href)) return item.href;
    }
    if (p.endsWith('/') || p.endsWith('/docs/') || p.endsWith('/docs')) return 'index.html';
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
          <svg class="sidebar-logo-icon" viewBox="0 0 200 200" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path fill-rule="evenodd" clip-rule="evenodd" d="M13.5969 23.2235C63.4924 -4.81833 123.53 2.55678 148.374 10.0273C153.006 11.4201 155.606 16.2198 154.181 20.7478C152.757 25.2758 147.846 27.8173 143.214 26.4245C120.866 19.7045 66.3988 13.3369 22.3442 38.0961C21.3748 38.6409 20.8298 39.6384 20.8808 40.6778C22.5544 74.7376 40.6646 139.735 103.059 173.709C107.293 176.015 108.813 181.238 106.455 185.377C104.096 189.516 98.7523 191.002 94.5186 188.696C24.7767 150.721 5.18199 78.7606 3.35124 41.5008C2.98177 33.9812 6.96683 26.9496 13.5969 23.2235Z" fill="#a5ff11"/>
            <path fill-rule="evenodd" clip-rule="evenodd" d="M180.214 22.5271C185.054 22.2785 189.183 25.9121 189.437 30.6429C192.776 92.7591 159.782 138.316 138.517 157.587C133.185 162.419 125.221 164.303 117.934 161.241C102.817 154.887 88.1835 141.52 76.3001 128.189C64.2403 114.66 54.1712 100.2 48.3006 90.492C45.8345 86.4139 47.2173 81.1537 51.3891 78.743C55.561 76.3323 60.9421 77.684 63.4082 81.7621C68.803 90.6832 78.2588 104.272 89.5315 116.919C100.981 129.763 113.488 140.698 124.866 145.48C125.179 145.612 125.851 145.671 126.586 145.006C145.206 128.132 174.903 87.1988 171.912 31.5431C171.657 26.8122 175.374 22.7756 180.214 22.5271Z" fill="#a5ff11"/>
            <path fill-rule="evenodd" clip-rule="evenodd" d="M85.9137 55.4487C70.263 57.0264 57.2007 60.7039 50.4014 63.4262C45.9168 65.2218 40.7922 63.1236 38.9553 58.7397C37.1184 54.3559 39.2649 49.3464 43.7495 47.5508C52.2083 44.164 66.9167 40.1173 84.1135 38.3837C101.323 36.649 121.62 37.1693 140.947 43.4423C149.244 46.1351 155.088 53.9217 154.482 62.8728C153.225 81.429 144.514 100.403 127.158 122.145C124.177 125.88 118.663 126.546 114.842 123.631C111.021 120.717 110.341 115.327 113.322 111.592C129.481 91.3484 136.032 75.6017 136.971 61.7394C137.02 61.0029 136.547 60.0904 135.417 59.7238C119.18 54.4537 101.552 53.8722 85.9137 55.4487Z" fill="#a5ff11"/>
          </svg>
          <div class="sidebar-logo-text">
            <strong>Calimero <em>mero-react</em></strong>
            <span>React SDK Bindings</span>
          </div>
        </div>
      </div>
      <div class="sidebar-search">
        <button class="docs-search-btn" id="open-search" aria-label="Search documentation">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/></svg>
          <span>Search docs…</span>
          <kbd>⌘K</kbd>
        </button>
        <input type="text" id="nav-search" placeholder="Filter pages…" autocomplete="off"/>
      </div>
      <div class="sidebar-nav" id="nav-links"></div>
      <div class="sidebar-footer">
        <div class="sidebar-footer-brand">© Calimero Network</div>
        <div class="sidebar-footer-links">
          <a href="https://github.com/calimero-network/mero-react" target="_blank" rel="noopener">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor"><path d="M12 2C6.477 2 2 6.477 2 12c0 4.418 2.865 8.166 6.839 9.489.5.092.682-.217.682-.482 0-.237-.009-.868-.013-1.703-2.782.604-3.369-1.342-3.369-1.342-.454-1.155-1.11-1.463-1.11-1.463-.908-.62.069-.608.069-.608 1.003.07 1.531 1.03 1.531 1.03.892 1.529 2.341 1.087 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.11-4.555-4.943 0-1.091.39-1.984 1.029-2.683-.103-.253-.446-1.27.098-2.647 0 0 .84-.269 2.75 1.025A9.578 9.578 0 0 1 12 6.836a9.59 9.59 0 0 1 2.504.337c1.909-1.294 2.747-1.025 2.747-1.025.546 1.377.202 2.394.1 2.647.64.699 1.028 1.592 1.028 2.683 0 3.842-2.339 4.687-4.566 4.935.359.309.678.919.678 1.852 0 1.336-.012 2.415-.012 2.743 0 .267.18.578.688.48C19.138 20.163 22 16.418 22 12c0-5.523-4.477-10-10-10z"/></svg>
            GitHub
          </a>
          <a href="https://calimero.network" target="_blank" rel="noopener">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor"><path d="M12 2a10 10 0 1 0 0 20A10 10 0 0 0 12 2zm-1 17.93V18a1 1 0 0 0-1-1H8a2 2 0 0 1-2-2v-1l-2.93-2.93A8.03 8.03 0 0 1 3 12c0-.62.073-1.22.21-1.8L6 13v1a2 2 0 0 0 2 2h1v1a2 2 0 0 0 2 2v.93zM17.9 17.4A2 2 0 0 0 16 16h-1v-3a1 1 0 0 0-1-1H9v-2h2a1 1 0 0 0 1-1V7h2a2 2 0 0 0 2-2v-.41A8.01 8.01 0 0 1 20 12a7.97 7.97 0 0 1-2.1 5.4z"/></svg>
            Website
          </a>
          <a href="https://docs.calimero.network" target="_blank" rel="noopener">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6zm-1 1.5L18.5 9H13V3.5zM6 20V4h5v7h7v9H6z"/></svg>
            Docs
          </a>
        </div>
      </div>
    `;
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
    btn.setAttribute('aria-label', 'Toggle navigation');
    btn.innerHTML = '<span>&#9776;</span>';
    btn.onclick = () => {
      const open = sb.classList.toggle('open');
      btn.innerHTML = open ? '<span>&#10005;</span>' : '<span>&#9776;</span>';
      btn.classList.toggle('sidebar-open', open);
    };
    document.body.prepend(btn);
    const navSearch = sb.querySelector('#nav-search');
    navSearch.addEventListener('input', () => {
      const q = navSearch.value.toLowerCase();
      linksEl.querySelectorAll('.nav-link').forEach(a => {
        a.style.display = a.dataset.label.includes(q) ? '' : 'none';
      });
      linksEl.querySelectorAll('.nav-section').forEach(s => {
        let hasVisible = false;
        let el = s.nextElementSibling;
        while (el && !el.classList.contains('nav-section')) {
          if (el.style.display !== 'none') hasVisible = true;
          el = el.nextElementSibling;
        }
        s.style.display = hasVisible ? '' : 'none';
      });
    });
    sb.querySelector('#open-search').addEventListener('click', openSearch);
  }

  let searchIndex = null, searchLoading = false, selectedIdx = -1;

  async function buildIndex() {
    if (searchIndex) return searchIndex;
    if (searchLoading) return null;
    searchLoading = true;
    const pages = NAV.filter(item => item.href);
    const results = await Promise.all(pages.map(async (page) => {
      try {
        const res = await fetch(PAGES_BASE + page.href);
        const html = await res.text();
        const parser = new DOMParser();
        const doc = parser.parseFromString(html, 'text/html');
        doc.querySelectorAll('script, style, nav, .sidebar, .menu-toggle').forEach(el => el.remove());
        const contentEl = doc.querySelector('.content') || doc.body;
        const headings = Array.from(contentEl.querySelectorAll('h1,h2,h3')).map(h => h.textContent.trim()).filter(Boolean);
        const text = contentEl.textContent.replace(/\s+/g, ' ').trim();
        return { label: page.label, href: page.href, dot: page.dot, text, headings };
      } catch { return null; }
    }));
    searchIndex = results.filter(Boolean);
    searchLoading = false;
    return searchIndex;
  }

  function escapeHtml(s) {
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  }

  function highlightMatch(text, query) {
    const q = String(query); if (!q.trim()) return escapeHtml(text);
    const t = String(text), qLower = q.toLowerCase(), tLower = t.toLowerCase();
    let out = '', i = 0;
    while (i < t.length) {
      const idx = tLower.indexOf(qLower, i);
      if (idx === -1) { out += escapeHtml(t.slice(i)); break; }
      out += escapeHtml(t.slice(i, idx)) + '<mark>' + escapeHtml(t.slice(idx, idx + q.length)) + '</mark>';
      i = idx + q.length;
    }
    return out;
  }

  function getExcerpt(text, query, ctxLen = 140) {
    const idx = text.toLowerCase().indexOf(query.toLowerCase());
    if (idx === -1) return null;
    const start = Math.max(0, idx - 60), end = Math.min(text.length, idx + ctxLen);
    return (start > 0 ? '…' : '') + highlightMatch(text.slice(start, end).trimStart(), query) + (end < text.length ? '…' : '');
  }

  function searchDocs(query, index) {
    const q = query.trim(); if (!q) return [];
    return index.map(page => {
      const titleMatch = page.label.toLowerCase().includes(q.toLowerCase());
      const headingMatch = page.headings.some(h => h.toLowerCase().includes(q.toLowerCase()));
      const excerpt = getExcerpt(page.text, q);
      if (!titleMatch && !headingMatch && !excerpt) return null;
      return { ...page, excerpt, score: (titleMatch?2:0)+(headingMatch?1:0)+(excerpt?.5:0) };
    }).filter(Boolean).sort((a,b)=>b.score-a.score).slice(0,8);
  }

  function renderResults(results, query) {
    const el = document.getElementById('search-results');
    if (!query.trim()) { el.innerHTML = '<p class="search-hint">Type to search across all documentation…</p>'; selectedIdx=-1; return; }
    if (!results.length) { el.innerHTML = '<p class="search-hint">No results for <strong>"'+escapeHtml(query)+'"</strong></p>'; selectedIdx=-1; return; }
    el.innerHTML = results.map((r,i) => `<a class="search-result-item" href="${PAGES_BASE+r.href}" data-idx="${i}"><div class="search-result-header"><span class="nav-dot" style="background:${r.dot};width:8px;height:8px;border-radius:2px;flex-shrink:0;display:inline-block;"></span><span class="search-result-title">${highlightMatch(r.label,query)}</span></div>${r.excerpt?`<p class="search-result-excerpt">${r.excerpt}</p>`:''}</a>`).join('');
    selectedIdx = -1;
  }

  function setSelected(idx, items) {
    items.forEach(el=>el.classList.remove('selected'));
    if (idx>=0&&idx<items.length){items[idx].classList.add('selected');items[idx].scrollIntoView({block:'nearest'});}
    selectedIdx=idx;
  }

  function openSearch() {
    let overlay = document.getElementById('search-overlay');
    if (!overlay) {
      overlay = document.createElement('div');
      overlay.id = 'search-overlay'; overlay.className = 'search-overlay';
      overlay.innerHTML = `<div class="search-modal" role="dialog" aria-modal="true"><div class="search-input-row"><svg class="search-icon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/></svg><input type="text" id="search-input" placeholder="Search documentation…" autocomplete="off" spellcheck="false"/><kbd class="search-esc-hint">Esc</kbd></div><div id="search-results" class="search-results"><p class="search-hint">Type to search across all documentation…</p></div><div class="search-footer-bar"><span><kbd>↑</kbd><kbd>↓</kbd> navigate</span><span><kbd>↵</kbd> open</span><span><kbd>Esc</kbd> close</span></div></div>`;
      document.body.appendChild(overlay);
      overlay.addEventListener('click', e => { if (e.target===overlay) closeSearch(); });
      const input = overlay.querySelector('#search-input');
      let debounceTimer;
      input.addEventListener('input', () => {
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(async () => {
          const q = input.value;
          if (!searchIndex&&!searchLoading) document.getElementById('search-results').innerHTML='<p class="search-hint search-loading">Loading index…</p>';
          const index = await buildIndex(); if (!index) return;
          renderResults(searchDocs(q,index),q);
        },120);
      });
      input.addEventListener('keydown', e => {
        const items = Array.from(document.querySelectorAll('.search-result-item'));
        if (e.key==='ArrowDown'){e.preventDefault();setSelected(Math.min(selectedIdx+1,items.length-1),items);}
        else if (e.key==='ArrowUp'){e.preventDefault();setSelected(Math.max(selectedIdx-1,0),items);}
        else if (e.key==='Enter'){if(selectedIdx>=0&&items[selectedIdx])window.location=items[selectedIdx].href;else if(items[0])window.location=items[0].href;}
        else if (e.key==='Escape') closeSearch();
      });
    }
    overlay.classList.add('open');
    setTimeout(()=>overlay.querySelector('#search-input').focus(),50);
    if (!searchIndex&&!searchLoading) buildIndex();
  }

  function closeSearch() {
    const overlay = document.getElementById('search-overlay');
    if (overlay) {
      overlay.classList.remove('open');
      const input = overlay.querySelector('#search-input');
      if (input) input.value='';
      const results = document.getElementById('search-results');
      if (results) results.innerHTML='<p class="search-hint">Type to search across all documentation…</p>';
    }
  }

  document.addEventListener('keydown', e => {
    if ((e.metaKey||e.ctrlKey)&&e.key==='k'){e.preventDefault();const o=document.getElementById('search-overlay');if(o&&o.classList.contains('open'))closeSearch();else openSearch();}
    if (e.key==='Escape') closeSearch();
  });

  function buildBreadcrumb(items) {
    const bc = document.querySelector('.breadcrumb'); if (!bc) return;
    bc.innerHTML = items.map((item,i)=>i===items.length-1?`<span>${item.label}</span>`:`<a href="${item.href}">${item.label}</a><span class="sep">/</span>`).join('');
  }

  function tabSystem() {
    document.querySelectorAll('[data-tabs]').forEach(container => {
      const tabs = container.querySelectorAll('.tab');
      const panels = container.parentElement.querySelectorAll('.panel');
      tabs.forEach(tab => { tab.addEventListener('click', () => { tabs.forEach(t=>t.classList.remove('on')); panels.forEach(p=>p.classList.remove('on')); tab.classList.add('on'); const target=document.getElementById(tab.dataset.target); if(target)target.classList.add('on'); }); });
    });
  }

  function ghLink(path, line) {
    const base = REPO + '/blob/main/';
    const url = line ? base+path+'#L'+line : base+path;
    return `<a class="gh-link" href="${url}" target="_blank" rel="noopener">${path}</a>`;
  }

  document.addEventListener('DOMContentLoaded', () => { buildSidebar(); tabSystem(); });
  window.arch = { ghLink, buildBreadcrumb, openSearch, REPO, PAGES_BASE };
})();
