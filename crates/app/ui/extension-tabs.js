// Extension pages (popup, options): the engine's chrome.tabs.create, chrome.windows.create and
// chrome.runtime.openOptionsPage open nothing in a WebView2 browser. Send them to Shoji instead,
// which shows the address in a tab.
(() => {
  if (location.protocol !== 'chrome-extension:' || window.top !== window) return;
  const post = url => {
    try { window.ipc.postMessage(JSON.stringify({ action: 'ExtensionOpen', payload: { url: String(url) } })); } catch (e) {}
  };
  const abs = u => { try { return new URL(String(u), location.href).href; } catch (e) { return ''; } };
  const put = (obj, name, fn) => {
    try { obj[name] = fn; } catch (e) {}
    if (obj[name] !== fn) { try { Object.defineProperty(obj, name, { value: fn, configurable: true, writable: true }); } catch (e) {} }
  };
  const done = (cb, value) => {
    if (typeof cb === 'function') { setTimeout(() => cb(value), 0); return undefined; }
    return Promise.resolve(value);
  };
  const urls = v => (Array.isArray(v) ? v : [v]).filter(Boolean).map(abs).filter(Boolean);
  const install = () => {
    const c = window.chrome;
    if (!c) return;
    if (c.tabs) {
      put(c.tabs, 'create', (props, cb) => {
        const u = urls(props && props.url);
        u.forEach(post);
        return done(cb, { id: -1, index: 0, windowId: -1, highlighted: true, active: true, pinned: false, incognito: false, url: u[0] || '' });
      });
    }
    if (c.windows) {
      put(c.windows, 'create', (props, cb) => {
        urls(props && props.url).forEach(post);
        return done(cb, { id: -1, focused: true, incognito: false, type: 'normal', state: 'normal', alwaysOnTop: false, tabs: [] });
      });
    }
    if (c.runtime) {
      put(c.runtime, 'openOptionsPage', cb => {
        let m = {};
        try { m = c.runtime.getManifest(); } catch (e) {}
        const page = (m.options_ui && m.options_ui.page) || m.options_page;
        if (page) post(abs('/' + String(page).replace(/^\//, '')));
        return done(cb, undefined);
      });
    }
  };
  install();
  addEventListener('DOMContentLoaded', install);
})();
