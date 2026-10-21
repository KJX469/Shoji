// Runs in the top frame of every web page (unless passwords or private mode are off). It does
// two things for Shoji's password manager and nothing else:
//   - reports a submitted login form, so the browser can offer to save it;
//   - reports where a login field is, so the browser can offer saved logins for this site.
// Nothing is filled until the user picks a login in the browser's own menu, and the browser only
// ever offers logins saved for the page's real address.
(() => {
  if (window.top !== window || !window.ipc) return;
  const send = (action, payload) => { try { window.ipc.postMessage(JSON.stringify({ action, payload })); } catch (e) {} };
  const TEXTY = /^(text|email|tel|)$/i;
  const isPassword = el => el && el.tagName === 'INPUT' && el.type === 'password';
  const visible = el => !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);

  // The username field for a password field: the closest visible text field before it.
  const usernameField = pw => {
    const scope = pw.form || document;
    let best = null;
    for (const i of scope.querySelectorAll('input')) {
      if (i === pw || !TEXTY.test(i.type) || i.disabled || !visible(i)) continue;
      if (i.compareDocumentPosition(pw) & Node.DOCUMENT_POSITION_FOLLOWING) best = i;
    }
    return best;
  };

  let last = '';
  const report = pw => {
    if (!pw.value || pw.value.length > 1024) return;
    const u = usernameField(pw);
    const username = u ? u.value.slice(0, 512) : '';
    const key = username + '\n' + pw.value;
    if (key === last) return;
    last = key;
    send('LoginSubmitted', { username, password: pw.value });
  };

  // A real submit, Enter in a password field, or a sign-in button (many sites log in by script).
  document.addEventListener('submit', e => {
    const pw = e.target && e.target.querySelector && e.target.querySelector('input[type=password]');
    if (pw) report(pw);
  }, true);
  document.addEventListener('keydown', e => { if (e.key === 'Enter' && isPassword(e.target)) report(e.target); }, true);
  document.addEventListener('click', e => {
    const b = e.target.closest && e.target.closest('button, input[type=submit], [role=button]');
    if (!b) return;
    const label = (b.innerText || b.value || b.getAttribute('aria-label') || '').slice(0, 40);
    const submits = b.type === 'submit' || (b.tagName === 'BUTTON' && !b.hasAttribute('type'));
    if (!submits && !/log ?in|sign ?in|submit|continue/i.test(label)) return;
    const pw = (b.form || document).querySelector('input[type=password]');
    if (pw && pw.value) report(pw);
  }, true);

  // Focusing a login field asks the browser for this site's saved logins.
  document.addEventListener('focusin', e => {
    const el = e.target;
    if (!el || el.tagName !== 'INPUT') return;
    const pw = isPassword(el) ? el : TEXTY.test(el.type) ? (el.form || document).querySelector('input[type=password]') : null;
    if (!pw) return;
    const r = el.getBoundingClientRect();
    send('LoginFieldFocused', { x: r.left, y: r.top, w: r.width, h: r.height });
  }, true);

  // Called by the browser after the user picks a login.
  const setValue = (el, v) => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, v);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };
  window.__shojiFill = (username, password) => {
    const pw = [...document.querySelectorAll('input[type=password]')].find(visible);
    if (!pw) return;
    const u = username && usernameField(pw);
    if (u) setValue(u, username);
    setValue(pw, password);
  };
})();
