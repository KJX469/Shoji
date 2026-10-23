// Chrome Web Store pages: the store's own "Add to Chrome" button asks Chrome (chrome.webstorePrivate)
// to install, which a WebView2 browser doesn't have, so the page spins forever. A real click on that
// button is handed to Shoji instead, which downloads and adds the extension itself.
(() => {
  if (window.top !== window) return;
  const host = location.hostname;
  if (host !== 'chromewebstore.google.com' && host !== 'chrome.google.com') return;
  addEventListener('click', e => {
    if (!e.isTrusted || !/\/detail\//.test(location.pathname)) return;
    // The store redraws the button between press and release, so the click can land on its wrapper.
    const t = e.target;
    const b = t && t.closest && (t.closest('button, [role="button"]') || t.querySelector('button, [role="button"]'));
    if (!b || !/chrome/i.test(b.textContent || '') || /remove/i.test(b.textContent || '')) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    try { window.ipc.postMessage(JSON.stringify({ action: 'StoreInstall', payload: null })); } catch (err) {}
  }, true);
})();
