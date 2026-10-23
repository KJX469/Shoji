// Chrome Web Store pages. Shoji installs extensions from a card on the page: it shows on every
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

  const root = document.createElement('div');
  root.style.cssText = 'all:initial;position:fixed;left:0;right:0;top:76px;z-index:2147483647;display:none;pointer-events:none;justify-content:center';
  const shadow = root.attachShadow({ mode: 'open' });
  shadow.innerHTML = `
    <style>
      .card { pointer-events: auto; display: flex; align-items: center; gap: 16px; box-sizing: border-box; width: min(560px, calc(100vw - 32px)); padding: 14px 18px;
        border-radius: 14px; background: #1f1d1a; color: #f6f1e8; box-shadow: 0 10px 34px rgba(0,0,0,.35), 0 0 0 1px rgba(255,255,255,.08);
        font: 14px/1.35 "Segoe UI", system-ui, sans-serif; }
      .mark { flex: none; width: 36px; height: 36px; border-radius: 9px; background: #e0533d; display: grid; place-items: center; font-weight: 700; font-size: 18px; color: #fff; }
      .text { flex: 1; min-width: 0; }
      .text b { display: block; font-size: 15px; }
      .text span { display: block; color: #cfc6b6; font-size: 12.5px; margin-top: 1px; }
      button { flex: none; cursor: pointer; border: 0; border-radius: 9px; padding: 10px 18px; font: 600 14px "Segoe UI", system-ui, sans-serif; background: #e0533d; color: #fff; }
      button:hover { filter: brightness(1.08); }
      button:disabled { cursor: default; opacity: .7; filter: none; }
      button.quiet { background: rgba(255,255,255,.12); color: #f6f1e8; }
      .err { color: #ffb4a6 !important; }
    </style>
    <div class="card" role="region" aria-label="Add to Shoji">
      <div class="mark" aria-hidden="true">S</div>
      <div class="text"><b id="t"></b><span id="s"></span></div>
      <button id="b"></button>
    </div>`;
  const $ = id => shadow.getElementById(id);
  let id = '', phase = 'ready', error = '', since = 0;

  const draw = () => {
    const b = $('b');
    $('s').className = '';
    b.className = '';
    b.disabled = false;
    if (phase === 'adding') {
      $('t').textContent = 'Adding to Shoji…';
      $('s').textContent = 'This takes a moment.';
      b.textContent = 'Adding…';
      b.disabled = true;
    } else if (phase === 'added') {
      $('t').textContent = 'Installed in Shoji';
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
      $('s').textContent = 'Install extensions into Shoji with this button.';
      b.textContent = 'Add to Shoji';
    }
  };
  $('b').onclick = () => {
    if (phase === 'added') { phase = 'adding'; $('t').textContent = 'Removing…'; send('StoreRemove'); }
    else { phase = 'adding'; since = Date.now(); send('StoreInstall'); }
    if (phase === 'adding') { $('b').disabled = true; }
  };
  // The host answers: where this extension stands.
  window.__shojiStore = (forId, next, message) => {
    if (forId !== id) return;
    phase = next; error = message || ''; draw();
  };

  const tick = () => {
    const now = idOf();
    const show = !!now && !isTheme();
    if (!root.isConnected && document.documentElement) document.documentElement.appendChild(root);
    root.style.display = show ? 'flex' : 'none';
    if (!show) { id = ''; return; }
    if (now !== id) {
      id = now; phase = 'ready'; error = ''; draw();
      send('StoreState');
    }
    // An install that never answers must not leave the card waiting.
    if (phase === 'adding' && since && Date.now() - since > 200000) {
      phase = 'error'; error = 'That took too long.'; draw();
    }
  };
  draw();
  setInterval(tick, 400);
  addEventListener('popstate', tick);
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
