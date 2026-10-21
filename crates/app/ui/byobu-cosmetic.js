// Byobu page runtime: applies a site's cosmetic filters. Injected at document start with the
// payload below, only in frames whose hostname matches the payload's host, and once per frame
// (a frame may get it at document start and again as a fallback once its DOM is ready).
(function (P) {
  'use strict';
  if (!P || location.hostname !== P.host || window.__byobu) return;
  Object.defineProperty(window, '__byobu', { value: true });
  var doc = document;
  var done = new WeakSet();

  /* ---- plain selectors: one adopted stylesheet, grouped rules with per-selector fallback ---- */
  var sheet = null, fallback = null;
  try {
    sheet = new CSSStyleSheet();
    doc.adoptedStyleSheets = [].concat(doc.adoptedStyleSheets || [], [sheet]);
  } catch (e) { sheet = null; }
  function insert(rule) {
    try {
      if (sheet) { sheet.insertRule(rule, sheet.cssRules.length); return true; }
      if (!fallback) {
        var host = doc.head || doc.documentElement;
        if (!host) return false;
        fallback = doc.createElement('style');
        host.appendChild(fallback);
      }
      fallback.sheet.insertRule(rule, fallback.sheet.cssRules.length);
      return true;
    } catch (e) { return false; }
  }
  var HIDE = '{display:none!important}';
  function hide(list) {
    for (var i = 0; i < list.length; i += 64) {
      var group = list.slice(i, i + 64);
      if (!insert(group.join(',') + HIDE)) {
        for (var j = 0; j < group.length; j++) insert(group[j] + HIDE);
      }
    }
  }
  if (P.hide && P.hide.length) hide(P.hide);

  /* ---- procedural filters ---- */
  function pattern(arg) {
    var m = /^\/(.+)\/([a-z]*)$/.exec(arg);
    try {
      if (m) return new RegExp(m[1], m[2]);
      return new RegExp('^' + arg.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*+/g, '.*') + '$', 's');
    } catch (e) { return null; }
  }
  function stripQuotes(s) { return s.replace(/^(["'])(.*)\1$/, '$2'); }
  function textPattern(arg) {
    var m = /^\/(.+)\/([a-z]*)$/.exec(arg);
    if (m) { try { return new RegExp(m[1], m[2]); } catch (e) { return null; } }
    return { test: function (t) { return t.indexOf(arg) !== -1; } };
  }

  var ops = {
    'has-text': function (set, arg) {
      var re = textPattern(arg);
      return re ? set.filter(function (e) { return re.test(e.textContent || ''); }) : [];
    },
    'min-text-length': function (set, arg) {
      var n = parseInt(arg, 10) || 0;
      return set.filter(function (e) { return (e.textContent || '').length >= n; });
    },
    'matches-css': function (set, arg, pe) {
      var i = arg.indexOf(':');
      if (i < 0) return [];
      var prop = arg.slice(0, i).trim(), re = pattern(stripQuotes(arg.slice(i + 1).trim()));
      return re ? set.filter(function (e) {
        return re.test(getComputedStyle(e, pe || null).getPropertyValue(prop));
      }) : [];
    },
    'matches-css-before': function (set, arg) { return ops['matches-css'](set, arg, '::before'); },
    'matches-css-after': function (set, arg) { return ops['matches-css'](set, arg, '::after'); },
    'matches-attr': function (set, arg) {
      var m = /^("[^"]*"|[^=]+)(?:=(.*))?$/.exec(arg.trim());
      if (!m) return [];
      var nameRe = pattern(stripQuotes(m[1])), valRe = m[2] !== undefined ? pattern(stripQuotes(m[2])) : null;
      if (!nameRe) return [];
      return set.filter(function (e) {
        return Array.prototype.some.call(e.attributes, function (a) {
          return nameRe.test(a.name) && (!valRe || valRe.test(a.value));
        });
      });
    },
    'matches-path': function (set, arg) {
      var re = pattern(arg);
      return re && re.test(location.pathname + location.search) ? set : [];
    },
    'upward': function (set, arg) {
      var out = [];
      set.forEach(function (e) {
        var n = /^\d+$/.test(arg) ? parseInt(arg, 10) : 0, p = e;
        if (n) { while (n-- > 0 && p) p = p.parentElement; }
        else { try { p = e.parentElement && e.parentElement.closest(arg); } catch (x) { p = null; } }
        if (p) out.push(p);
      });
      return out;
    },
    'xpath': function (set, arg) {
      var out = [];
      set.forEach(function (e) {
        try {
          var r = doc.evaluate(arg, e, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null);
          for (var i = 0; i < r.snapshotLength; i++) {
            var n = r.snapshotItem(i);
            if (n && n.nodeType === 1) out.push(n);
          }
        } catch (x) { /* bad expression */ }
      });
      return out;
    },
  };
  function evaluate(steps) {
    var set = null;
    for (var i = 0; i < steps.length; i++) {
      var s = steps[i];
      if (s.type === 'css-selector') {
        try {
          if (set === null) set = Array.prototype.slice.call(doc.querySelectorAll(s.arg));
          else {
            var rel = /^[>+~]/.test(s.arg.trim()) ? ':scope' + s.arg : ':scope ' + s.arg, next = [];
            set.forEach(function (e) {
              try { Array.prototype.push.apply(next, e.querySelectorAll(rel)); } catch (x) { /* skip */ }
            });
            set = next;
          }
        } catch (x) { return []; }
      } else {
        if (set === null) set = Array.prototype.slice.call(doc.querySelectorAll('*'));
        var fn = ops[s.type];
        if (!fn) return [];
        set = fn(set, s.arg);
      }
      if (!set.length) return [];
    }
    return set || [];
  }
  function act(el, action) {
    if (done.has(el)) return;
    done.add(el);
    var t = action && action.type;
    try {
      if (t === 'remove') el.remove();
      else if (t === 'style') el.style.cssText += ';' + action.arg;
      else if (t === 'remove-attr') el.removeAttribute(action.arg);
      else if (t === 'remove-class') el.classList.remove(action.arg);
      else el.style.setProperty('display', 'none', 'important');
    } catch (x) { /* detached */ }
  }
  var procedural = (P.procedural || []).filter(function (f) { return f && f.selector && f.selector.length; });
  function runProcedural() {
    for (var i = 0; i < procedural.length; i++) {
      var f = procedural[i];
      evaluate(f.selector).forEach(function (el) { act(el, f.action); });
    }
  }

  /* ---- generic class/id rules: ask the host which selectors apply to what the page uses ---- */
  var seenC = Object.create(null), seenI = Object.create(null), qC = [], qI = [];
  function scan(root) {
    var nodes;
    try {
      nodes = root.querySelectorAll('[class],[id]');
    } catch (e) { return; }
    var list = root.nodeType === 1 ? [root].concat(Array.prototype.slice.call(nodes)) : nodes;
    for (var i = 0; i < list.length; i++) {
      var n = list[i];
      if (n.id && !seenI[n.id]) { seenI[n.id] = 1; qI.push(n.id); }
      var cl = n.classList;
      if (cl) for (var j = 0; j < cl.length; j++) {
        if (!seenC[cl[j]]) { seenC[cl[j]] = 1; qC.push(cl[j]); }
      }
    }
  }
  function ask() {
    if (!qC.length && !qI.length) return;
    var c = qC.splice(0, 1000), i = qI.splice(0, 1000);
    try { window.chrome.webview.postMessage('byobu:' + JSON.stringify({ c: c, i: i })); } catch (e) { /* no bridge */ }
  }
  if (P.generic) {
    Object.defineProperty(window, '__byobuHide', {
      value: function (list) { if (Array.isArray(list) && list.length) hide(list); },
    });
  }

  /* ---- scheduling ---- */
  var timer = 0, pending = [];
  function work() {
    timer = 0;
    var roots = pending; pending = [];
    if (procedural.length) runProcedural();
    if (P.generic) { roots.forEach(scan); ask(); }
  }
  function schedule(root) {
    pending.push(root || doc);
    if (!timer) timer = setTimeout(work, 150);
  }
  function start() {
    schedule(doc);
    new MutationObserver(function (muts) {
      for (var i = 0; i < muts.length; i++) {
        var m = muts[i];
        if (m.type === 'attributes') { pending.push(m.target); continue; }
        for (var j = 0; j < m.addedNodes.length; j++) {
          if (m.addedNodes[j].nodeType === 1) pending.push(m.addedNodes[j]);
        }
      }
      if (!timer) timer = setTimeout(work, 150);
    }).observe(doc, { childList: true, subtree: true, attributes: true, attributeFilter: ['class', 'id'] });
  }
  if (doc.readyState === 'loading') doc.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})(__BYOBU_PAYLOAD__);
