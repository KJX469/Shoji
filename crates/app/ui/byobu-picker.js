// Byobu element picker: hover to outline, click to choose, then hide it for good.
// Injected by the host on request; __TOKEN__ authorises the single reply.
(function (TOKEN) {
  'use strict';
  if (window.__byobuPick) { try { window.__byobuPick(); } catch (e) { /* stale */ } }
  var doc = document, root = doc.documentElement;
  var box = doc.createElement('div'), bar = doc.createElement('div');
  var Z = '2147483647';
  box.style.cssText = 'position:fixed;pointer-events:none;z-index:' + Z + ';border:2px solid #d96b3b;background:rgba(217,107,59,.16);border-radius:3px;display:none;left:0;top:0;width:0;height:0;transition:none';
  bar.style.cssText = 'position:fixed;left:50%;bottom:20px;transform:translateX(-50%);z-index:' + Z + ';display:flex;gap:8px;align-items:center;max-width:min(92vw,720px);padding:9px 12px;border-radius:12px;background:#26231f;color:#f4efe6;font:13px/1.35 system-ui,sans-serif;box-shadow:0 6px 24px rgba(0,0,0,.35)';
  var label = doc.createElement('span');
  label.style.cssText = 'min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap';
  label.textContent = 'Click the element you want to hide';
  function btn(text, fn) {
    var b = doc.createElement('button');
    b.textContent = text;
    b.style.cssText = 'all:unset;cursor:pointer;padding:5px 10px;border-radius:7px;background:#f4efe6;color:#26231f;font:13px system-ui,sans-serif';
    b.addEventListener('click', function (e) { e.preventDefault(); e.stopPropagation(); fn(); }, true);
    return b;
  }
  var chosen = null, hover = null;
  var up = btn('Larger', function () { if (chosen && chosen.parentElement && chosen.parentElement !== doc.body) choose(chosen.parentElement); });
  var ok = btn('Hide', function () { commit(); });
  var cancel = btn('Cancel', function () { stop(); });
  [up, ok].forEach(function (b) { b.style.display = 'none'; });
  bar.appendChild(label); bar.appendChild(up); bar.appendChild(ok); bar.appendChild(cancel);
  root.appendChild(box); root.appendChild(bar);

  function inUi(el) { return el === bar || bar.contains(el) || el === box; }
  function show(el) {
    var r = el.getBoundingClientRect();
    box.style.display = 'block';
    box.style.left = r.left + 'px'; box.style.top = r.top + 'px';
    box.style.width = Math.max(r.width, 2) + 'px'; box.style.height = Math.max(r.height, 2) + 'px';
  }
  function esc(s) { return window.CSS && CSS.escape ? CSS.escape(s) : s.replace(/[^\w-]/g, '\\$&'); }
  function unique(sel) { try { return doc.querySelectorAll(sel).length === 1; } catch (e) { return false; } }
  function stableClass(c) { return c && c.length < 40 && !/\d{3,}/.test(c) && !/^[a-z]{1,2}[A-Z0-9_-]{5,}$/.test(c); }
  function selectorFor(el) {
    var parts = [], node = el;
    for (var depth = 0; node && node.nodeType === 1 && node !== doc.documentElement && depth < 7; depth++) {
      var part = node.tagName.toLowerCase();
      if (node.id && !/\d{4,}/.test(node.id)) {
        part = '#' + esc(node.id);
        parts.unshift(part);
        if (unique(parts.join(' > '))) return parts.join(' > ');
        break;
      }
      var cls = Array.prototype.filter.call(node.classList || [], stableClass).slice(0, 3);
      if (cls.length) part += '.' + cls.map(esc).join('.');
      var p = node.parentElement;
      if (p && p.querySelectorAll(':scope > ' + part).length > 1) {
        var idx = Array.prototype.indexOf.call(p.children, node) + 1;
        part += ':nth-child(' + idx + ')';
      }
      parts.unshift(part);
      if (unique(parts.join(' > '))) return parts.join(' > ');
      node = p;
    }
    return parts.join(' > ');
  }
  function choose(el) {
    chosen = el;
    show(el);
    var sel = selectorFor(el);
    var n = 0;
    try { n = doc.querySelectorAll(sel).length; } catch (e) { /* invalid */ }
    label.textContent = sel + (n > 1 ? '  (matches ' + n + ')' : '');
    label.title = sel;
    up.style.display = '';
    // Hiding most of the page (a feed container, say) would blank the site for good.
    var r = el.getBoundingClientRect();
    var huge = r.width >= innerWidth * 0.8 && r.height >= innerHeight * 0.6;
    if (huge) label.textContent = 'Too large to hide: this covers most of the page';
    ok.style.display = huge ? 'none' : '';
  }
  function onMove(e) {
    if (chosen) return;
    var el = doc.elementFromPoint(e.clientX, e.clientY);
    if (!el || inUi(el) || el === doc.documentElement || el === doc.body) return;
    hover = el; show(el);
  }
  function swallow(e) { if (!inUi(e.target)) { e.preventDefault(); e.stopPropagation(); } }
  function onClick(e) {
    if (inUi(e.target)) return;
    e.preventDefault(); e.stopPropagation();
    var el = hover || e.target;
    if (el && el !== doc.documentElement && el !== doc.body) choose(el);
  }
  function onKey(e) { if (e.key === 'Escape') { e.preventDefault(); stop(); } }
  function commit() {
    if (!chosen) return;
    var cr = chosen.getBoundingClientRect();
    if (cr.width >= innerWidth * 0.8 && cr.height >= innerHeight * 0.6) return;
    var sel = selectorFor(chosen);
    try {
      var st = doc.createElement('style');
      st.textContent = sel + '{display:none!important}';
      (doc.head || root).appendChild(st);
    } catch (e) { chosen.style.setProperty('display', 'none', 'important'); }
    try { window.chrome.webview.postMessage('byobu:rule:' + JSON.stringify({ t: TOKEN, s: sel })); } catch (e) { /* no bridge */ }
    stop();
  }
  function stop() {
    doc.removeEventListener('mousemove', onMove, true);
    doc.removeEventListener('click', onClick, true);
    ['mousedown', 'mouseup', 'pointerdown', 'pointerup', 'auxclick', 'contextmenu'].forEach(function (t) { doc.removeEventListener(t, swallow, true); });
    doc.removeEventListener('keydown', onKey, true);
    box.remove(); bar.remove();
    window.__byobuPick = null;
  }
  doc.addEventListener('mousemove', onMove, true);
  doc.addEventListener('click', onClick, true);
  ['mousedown', 'mouseup', 'pointerdown', 'pointerup', 'auxclick', 'contextmenu'].forEach(function (t) { doc.addEventListener(t, swallow, true); });
  doc.addEventListener('keydown', onKey, true);
  window.__byobuPick = stop;
})('__TOKEN__');
