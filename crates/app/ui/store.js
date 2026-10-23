// Chrome Web Store pages. Shoji installs extensions from a banner on the page: it shows on every
// extension's page, as the store moves between them, and runs the install itself. The store's own
// "Add to Chrome" button asks Chrome (chrome.webstorePrivate) to install, which a WebView2 browser
// doesn't have, so a real click on it is handed to Shoji too.
(() => {
  if (window.top !== window) return;
  const host = location.hostname;
  if (host !== 'chromewebstore.google.com' && host !== 'chrome.google.com') return;
  const send = action => { try { window.ipc.postMessage(JSON.stringify({ action, payload: null })); } catch (e) {} };
  const idOf = () => {
    const m = location.pathname.match(/\/detail\/(?:[^/]+\/)?([a-p]{32})(?:[/?#]|$)/);
    return m ? m[1] : '';
  };
  // Themes have detail pages of their own; they are not extensions.
  const isTheme = () => !!document.querySelector('a[href*="/category/theme"]');

  const BAR = 74, GAP = 14;
  const CRANE = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='262 242 720 720' role='img' aria-label='Shoji'%3E%3Cdefs%3E%3ClinearGradient id='p' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop offset='0' stop-color='%238BB4FF'/%3E%3Cstop offset='1' stop-color='%23397AFF'/%3E%3C/linearGradient%3E%3ClinearGradient id='f' x1='0' y1='0' x2='1' y2='1'%3E%3Cstop offset='0' stop-color='%232054D3'/%3E%3Cstop offset='1' stop-color='%23173A9C'/%3E%3C/linearGradient%3E%3C/defs%3E%3Cpath d='M282 547 515 653 517 827Z' fill='url(%23p)'/%3E%3Cpath d='M529 334 731 660 529 869Z' fill='url(%23p)'/%3E%3Cpath d='M529 869 731 660 791 767Z' fill='url(%23f)'/%3E%3Cpath d='M731 660 860 540Q865 535 870 540L962 599 862 586 873 547 865 543 790 766Z' fill='url(%23p)'/%3E%3C/svg%3E";
  const root = document.createElement('div');
  root.style.cssText = 'all:initial;position:fixed;left:0;right:0;top:64px;height:' + BAR + 'px;z-index:2147483647;display:none;pointer-events:none';
  const shadow = root.attachShadow({ mode: 'open' });
  // The store's pages only take trusted HTML, so the banner is built element by element.
  const el = (tag, props, ...kids) => { const e = document.createElement(tag); Object.assign(e, props); e.append(...kids); return e; };
  const style = el('style');
  style.textContent = `
    :host { color-scheme: light dark; }
    .bar { --bg: #14264d; --edge: #4f86ff; --ink: #f1f5ff; --sub: #b9c8ea; --btn: #2b63e8; --btn-hover: #2457d6; --btn-ink: #fff;
      --quiet: transparent; --quiet-ink: #f1f5ff; --quiet-edge: #8fa8e6; --err: #ffb4a6; --shadow: 0 6px 22px rgba(0,0,0,.55);
      pointer-events: auto; box-sizing: border-box; display: flex; align-items: center; gap: 16px; width: min(1040px, calc(100vw - 32px)); height: 62px; margin: 6px auto 0; padding: 0 16px 0 18px;
      background: var(--bg); color: var(--ink); border: 1px solid var(--edge); border-left: 5px solid var(--edge); border-radius: 14px;
      box-shadow: var(--shadow), 0 0 0 1px rgba(255,255,255,.04); font: 14px/1.3 "Segoe UI", system-ui, sans-serif; }
    .bar[data-theme="light"] { --bg: #ffffff; --edge: #2b63e8; --ink: #0d1a36; --sub: #3b4a6b; --btn: #2b63e8; --btn-hover: #2457d6;
      --quiet: transparent; --quiet-ink: #0d1a36; --quiet-edge: #6f8fd9; --err: #b3261e; --shadow: 0 6px 18px rgba(13,26,54,.22); }
    img { flex: none; width: 40px; height: 40px; }
    .ok { flex: none; width: 22px; height: 22px; color: #5fe0a0; }
    .bar[data-theme="light"] .ok { color: #137a45; }
    .text { flex: 1; min-width: 0; }
    .text b { display: flex; align-items: center; gap: 8px; font-size: 17px; font-weight: 650; }
    .text span { display: block; color: var(--sub); font-size: 14px; margin-top: 2px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .text span.err { color: var(--err); font-weight: 600; }
    button { flex: none; display: inline-flex; align-items: center; gap: 8px; min-height: 38px; cursor: pointer; border: 1px solid transparent; border-radius: 10px; padding: 0 22px;
      font: 650 15px "Segoe UI", system-ui, sans-serif; background: var(--btn); color: var(--btn-ink); }
    button:hover { background: var(--btn-hover); }
    button:focus-visible { outline: 3px solid var(--ink); outline-offset: 2px; }
    button:disabled { cursor: default; background: var(--btn); }
    button.quiet { background: var(--quiet); color: var(--quiet-ink); border-color: var(--quiet-edge); }
    button.quiet:hover { background: rgba(120,150,230,.22); }
    .spin { width: 14px; height: 14px; border-radius: 50%; border: 2px solid rgba(255,255,255,.4); border-top-color: #fff; animation: spin .8s linear infinite; }
    @keyframes spin { to { transform: rotate(360deg); } }
    @media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
    @media (max-width: 560px) { .text span { display: none; } button { padding: 0 14px; } }
  `;
  const bar = el('div', { className: 'bar' },
    el('img', { src: CRANE, alt: '' }),
    el('div', { className: 'text' }, el('b', { id: 't' }), el('span', { id: 's' })),
    el('button', { id: 'b' }));
  bar.setAttribute('role', 'region');
  bar.setAttribute('aria-label', 'Add to Shoji');
  shadow.append(style, bar);
  const $ = id => shadow.getElementById(id);
  let id = '', phase = 'ready', error = '', since = 0;

  // A check mark, built as SVG nodes (the store only takes trusted HTML).
  const tick_ = () => {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('class', 'ok');
    svg.setAttribute('aria-hidden', 'true');
    const c = document.createElementNS(ns, 'circle');
    c.setAttribute('cx', '12'); c.setAttribute('cy', '12'); c.setAttribute('r', '11'); c.setAttribute('fill', 'currentColor');
    const p = document.createElementNS(ns, 'path');
    p.setAttribute('d', 'M7 12.5l3.2 3.2L17 8.8'); p.setAttribute('fill', 'none'); p.setAttribute('stroke', '#0d1a36'); p.setAttribute('stroke-width', '2.4');
    p.setAttribute('stroke-linecap', 'round'); p.setAttribute('stroke-linejoin', 'round');
    svg.append(c, p);
    return svg;
  };

  const draw = () => {
    const b = $('b');
    $('s').className = '';
    b.className = '';
    b.disabled = false;
    b.textContent = '';
    if (phase === 'adding') {
      $('t').textContent = 'Adding to Shoji…';
      $('s').textContent = 'This takes a moment.';
      b.append(el('span', { className: 'spin' }), 'Adding');
      b.disabled = true;
    } else if (phase === 'added') {
      $('t').textContent = 'Installed in Shoji';
      $('t').prepend(tick_());
      $('s').textContent = 'Find it under the puzzle piece in the toolbar.';
      b.textContent = 'Remove';
      b.className = 'quiet';
    } else if (phase === 'error') {
      $('t').textContent = 'That didn’t work.';
      $('s').textContent = error || 'Try again.';
      $('s').className = 'err';
      b.textContent = 'Try again';
    } else {
      $('t').textContent = 'Add to Shoji';
      $('s').textContent = 'Install extensions in Shoji with this button for now.';
      b.textContent = 'Add to Shoji';
    }
  };
  $('b').onclick = () => {
    if (phase === 'added') { phase = 'adding'; draw(); $('t').textContent = 'Removing…'; send('StoreRemove'); }
    else { phase = 'adding'; since = Date.now(); draw(); send('StoreInstall'); }
  };
  // The host answers: where this extension stands.
  window.__shojiStore = (forId, next, message) => {
    if (forId !== id) return;
    phase = next; error = message || ''; draw();
  };

  // Dark or light: what the page itself is painted with, else the system's setting.
  const dark = () => {
    for (const e of [document.body, document.documentElement]) {
      const m = e && getComputedStyle(e).backgroundColor.match(/[\d.]+/g);
      if (m && m.length >= 3 && !(m.length > 3 && +m[3] === 0)) return (0.299 * m[0] + 0.587 * m[1] + 0.114 * m[2]) < 140;
    }
    return matchMedia('(prefers-color-scheme: dark)').matches;
  };
  // Under the store's own header, with the page pushed down by the banner's height.
  const place = show => {
    let top = 0;
    const h = document.querySelector('header, [role="banner"]');
    if (h) { const r = h.getBoundingClientRect(); if (r.height > 0 && r.top <= 1) top = Math.round(r.bottom); }
    root.style.top = (top + 4) + 'px';
    document.body && (document.body.style.paddingTop = show ? (BAR + GAP) + 'px' : '');
  };

  const tick = () => {
    const now = idOf();
    const show = !!now && !isTheme();
    if (!root.isConnected && document.documentElement) document.documentElement.appendChild(root);
    root.style.display = show ? 'block' : 'none';
    place(show);
    if (!show) { id = ''; return; }
    bar.dataset.theme = dark() ? 'dark' : 'light';
    if (now !== id) {
      id = now; phase = 'ready'; error = ''; draw();
      send('StoreState');
    }
    // An install that never answers must not leave the banner waiting.
    if (phase === 'adding' && since && Date.now() - since > 200000) {
      phase = 'error'; error = 'That took too long.'; draw();
    }
  };
  draw();
  setInterval(tick, 400);
  addEventListener('popstate', tick);
  addEventListener('resize', tick);
  addEventListener('DOMContentLoaded', tick);
  tick();

  addEventListener('click', e => {
    if (!e.isTrusted || !idOf()) return;
    // The store redraws the button between press and release, so the click can land on its wrapper.
    const t = e.target;
    if (t && t.getRootNode && t.getRootNode() === shadow) return;
    const b = t && t.closest && (t.closest('button, [role="button"]') || t.querySelector('button, [role="button"]'));
    if (!b || !/chrome/i.test(b.textContent || '') || /remove/i.test(b.textContent || '')) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (phase !== 'adding') { phase = 'adding'; since = Date.now(); draw(); send('StoreInstall'); }
  }, true);
})();
