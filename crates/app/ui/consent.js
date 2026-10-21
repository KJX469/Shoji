// Cookie notices answered with "Reject all": every consent category is turned off and the choice
// saved, never "Accept". The rules are Consent-O-Matic's (MIT, https://github.com/cavi-au/Consent-O-Matic),
// and this runs them the way its engine does. Detection watches the page for a few seconds after
// it loads; a notice that shows asks the host for its steps, which arrive through `run`.
(function (DETECTORS) {
  'use strict';
  if (window.__shojiConsent) return;
  var post = window.chrome && chrome.webview && chrome.webview.postMessage.bind(chrome.webview);
  if (!post) return;
  var WATCH_MS = 8000;
  // Every category off: reject.
  var CHOICES = { A: false, B: false, D: false, E: false, F: false, X: false };

  var base = null;
  var topUrl = location.href;
  try {
    var anc = location.ancestorOrigins;
    if (anc && anc.length) topUrl = anc[anc.length - 1] + '/';
  } catch (x) { /* none */ }

  /* ---- finding elements (Consent-O-Matic's target and parent options) ---- */
  var spaces = /\s{2,}/gm;
  function findElement(o, parent, multiple) {
    var found;
    if (o.selector.trim() === ':scope') {
      found = [parent || base || document];
    } else {
      var root = parent || base || document;
      if (root.shadowRoot) root = root.shadowRoot;
      found = Array.prototype.slice.call(root.querySelectorAll(o.selector));
    }
    if (o.textFilter != null) {
      var texts = [].concat(o.textFilter).map(function (t) { return t.toLowerCase().replace(spaces, ' '); });
      found = found.filter(function (el) {
        var text = el.textContent.toLowerCase().replace(spaces, ' ');
        return texts.some(function (t) { return text.indexOf(t) !== -1; });
      });
    }
    if (o.styleFilters != null) {
      found = found.filter(function (el) {
        var st = getComputedStyle(el);
        return o.styleFilters.every(function (f) { return (st[f.option] === f.value) !== !!f.negated; });
      });
    }
    if (o.displayFilter != null) {
      found = found.filter(function (el) {
        if (el.matches('.shoji-cmp-nodetect')) return !o.displayFilter;
        return (el.offsetHeight !== 0) === !!o.displayFilter;
      });
    }
    if (o.iframeFilter != null) {
      var framed = window !== window.top;
      found = found.filter(function () { return framed === !!o.iframeFilter; });
    }
    if (o.childFilter != null) {
      found = found.filter(function (el) {
        var old = base;
        base = el;
        var hit = find(o.childFilter).target != null;
        base = old;
        return hit !== !!o.childFilterNegated;
      });
    }
    return multiple ? found : found[0];
  }

  function find(o, multiple) {
    var out = [];
    var add = function (parent, targets) {
      [].concat(targets).forEach(function (t) { out.push({ parent: parent, target: t }); });
    };
    if (o.parent != null) {
      var parents = findElement(o.parent, null, multiple);
      if (parents != null) {
        [].concat(parents).forEach(function (p) { add(p, findElement(o.target, p, multiple)); });
      }
    } else {
      add(null, findElement(o.target, null, multiple));
    }
    if (!out.length) out.push({ parent: null, target: null });
    return multiple ? out : out[0];
  }

  /* ---- matchers ---- */
  function matches(m) {
    switch (m.type) {
      case 'css': return find(m).target != null;
      case 'checkbox': {
        var box = find(m).target;
        if (box == null) throw new Error('no checkbox');
        return m.negated ? !box.checked : !!box.checked;
      }
      case 'url': {
        var hit = [].concat(m.url).some(function (u) {
          return m.regexp ? new RegExp(u).test(topUrl) : topUrl.indexOf(u) > -1;
        });
        return m.negated ? !hit : hit;
      }
      case 'onoff': {
        var on = find(m.onMatcher).target != null, off = find(m.offMatcher).target != null;
        if (on === off) throw new Error('on and off');
        return on;
      }
      default: throw new Error('matcher ' + m.type);
    }
  }
  function all(list) {
    list = [].concat(list).filter(Boolean);
    return list.every(function (m) { try { return matches(m); } catch (x) { return false; } });
  }
  function detected(d) { var p = [].concat(d.presentMatcher).filter(Boolean); return p.length > 0 && all(p); }

  /* ---- actions ---- */
  var wait = function (ms) { return new Promise(function (r) { setTimeout(r, ms); }); };
  var hidden = [];
  var clicks = 0;

  async function act(a, cmp) {
    if (!a) return;
    switch (a.type) {
      case 'click': {
        var t = find(a).target;
        if (t) { t.click(); clicks++; await wait(a.timeout || 0); }
        return;
      }
      case 'multiclick':
        find(a, true).forEach(function (r) { if (r.target) { r.target.click(); clicks++; } });
        return;
      case 'list':
        for (var i = 0; i < a.actions.length; i++) await act(a.actions[i], cmp);
        return;
      case 'consent':
        for (var j = 0; j < a.consents.length; j++) await consent(a.consents[j], cmp);
        return;
      case 'ifcss':
        return act(find(a).target != null ? a.trueAction : a.falseAction, cmp);
      case 'waitcss': {
        var tries = a.retries || 10, every = a.waitTime || 250;
        while ((find(a).target != null) === !!a.negated && tries-- > 0) await wait(every);
        return;
      }
      case 'foreach': {
        var old = base;
        var rs = find(a, true);
        for (var k = 0; k < rs.length; k++) {
          if (rs[k].target) { base = rs[k].target; await act(a.action, cmp); }
        }
        base = old;
        return;
      }
      case 'hide': {
        var h = find(a).target;
        if (h) {
          if (a.hideFromDetection) h.classList.add('shoji-cmp-nodetect');
          h.classList.add('shoji-cmp-hidden');
          hidden.push(h);
        }
        return;
      }
      case 'slide': {
        var s = find(a).target, to = find(a.dragTarget).target;
        if (!s || !to) return;
        var sb = s.getBoundingClientRect(), tb = to.getBoundingClientRect();
        var dx = a.axis && a.axis.toLowerCase() === 'y' ? 0 : tb.left - sb.left;
        var dy = a.axis && a.axis.toLowerCase() === 'x' ? 0 : tb.top - sb.top;
        var x = sb.left + sb.width / 2, y = sb.top + sb.height / 2;
        var ev = function (type, ex, ey) {
          s.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, clientX: ex, clientY: ey }));
        };
        ev('mousedown', x, y); await wait(10);
        ev('mousemove', x + dx, y + dy); await wait(10);
        ev('mouseup', x + dx, y + dy);
        clicks++;
        return;
      }
      case 'wait': return wait(a.waitTime || 0);
      case 'ifallowall': return act(a.falseAction, cmp);
      case 'ifallownone': return act(a.trueAction, cmp);
      case 'runrooted': {
        var prev = base;
        if (a.ignoreOldRoot) base = null;
        var root = find(a).target;
        if (root) { base = root; await act(a.action, cmp); }
        base = prev;
        return;
      }
      case 'runmethod': {
        var m = cmp[(a.method || '').toUpperCase()];
        if (m && m.custom) await act(m.action, cmp);
        return;
      }
      // "close" closes the window: never.
      default: return;
    }
  }

  async function consent(c, cmp) {
    var on = !!CHOICES[c.type];
    if (c.toggleAction) {
      if (!c.matcher) return;
      try { if (matches(c.matcher) !== on) await act(c.toggleAction, cmp); } catch (x) { /* state unknown */ }
      return;
    }
    if (c.matcher && c.matcher.type === 'onoff') {
      try {
        var cur = matches(c.matcher);
        if (cur && !on) await act(c.falseAction, cmp);
        else if (!cur && on) await act(c.trueAction, cmp);
        return;
      } catch (x) { /* fall through */ }
    }
    await act(on ? c.trueAction : c.falseAction, cmp);
  }

  /* ---- watching the page ---- */
  // Most detectors start by looking for one element. A style rule that starts a tiny animation on
  // any of those elements lets the style engine, which indexes selectors by id and class, report
  // when one appears or becomes visible; a page without a notice costs next to nothing to watch.
  // The few detectors that can't be keyed this way, and elements whose own animation wins over
  // the rule, are caught by a handful of full scans.
  var SEEN = 'shoji-cmp-seen';
  var FULL_SCANS = [0, 2500, 6000];
  var tried = {};
  var busy = false;
  var stopAt = Date.now() + WATCH_MS;
  var queued = 0, timers = [];

  var frag = document.createDocumentFragment();
  function keyOf(d) {
    var m = [].concat(d.presentMatcher).filter(Boolean)[0];
    var o = m && m.type === 'css' && (m.parent || m.target);
    if (!o || !o.selector || o.selector.trim() === ':scope') return null;
    try { frag.querySelector(o.selector); } catch (x) { return null; }
    return o.selector;
  }
  var checks = [];
  for (var cmpName in DETECTORS) {
    DETECTORS[cmpName].forEach(function (d) { checks.push({ name: cmpName, d: d, key: keyOf(d) }); });
  }

  function scan() {
    queued = 0;
    if (busy || Date.now() > stopAt) return;
    for (var i = 0; i < checks.length; i++) {
      var c = checks[i], name = c.name;
      if (tried[name]) continue;
      if (c.key && document.querySelector(c.key) == null) continue;
      if (!detected(c.d)) continue;
      tried[name] = true;
      busy = name;
      whenShowing(name, c.d, 5);
      return;
    }
  }
  function soon() { if (!queued) queued = setTimeout(scan, 100); }
  function whenShowing(name, d, tries) {
    if (all(d.showingMatcher || [])) {
      post('shoji:consent:cmp:' + name);
      // The steps come back through run(); if they never do, watching goes on.
      setTimeout(function () { if (busy === name) { busy = false; soon(); } }, 3000);
      return;
    }
    if (tries > 0) { setTimeout(function () { whenShowing(name, d, tries - 1); }, 250); return; }
    busy = false;
  }
  var started = false;
  function start() {
    if (started) return;
    started = true;
    var keys = checks.map(function (c) { return c.key; }).filter(Boolean);
    var css = document.createElement('style');
    css.textContent = '@keyframes ' + SEEN + '{from{opacity:.99}to{opacity:1}}' +
      keys.filter(function (k, i) { return keys.indexOf(k) === i; }).join(',') +
      '{animation:' + SEEN + ' 1ms!important}' +
      '.shoji-cmp-hidden{opacity:0!important;pointer-events:none!important}';
    (document.head || document.documentElement).appendChild(css);
    document.addEventListener('animationstart', function (e) { if (e.animationName === SEEN) soon(); }, true);
    FULL_SCANS.forEach(function (ms) { timers.push(setTimeout(scan, ms)); });
    // Watching ends after a while; a notice the page shows later is left to the user.
    timers.push(setTimeout(function () { css.textContent = '.shoji-cmp-hidden{opacity:0!important;pointer-events:none!important}'; }, WATCH_MS));
  }

  window.__shojiConsent = {
    run: async function (name, methods) {
      if (busy !== name) return;
      var cmp = {};
      methods.forEach(function (m) { if (m.action) cmp[m.name] = { action: m.action, custom: m.custom === true }; });
      var only = Object.keys(cmp);
      var utility = only.length === 1 && only[0] === 'UTILITY';
      clicks = 0;
      try {
        if (utility) {
          await act(cmp.UTILITY.action, cmp);
        } else {
          for (var step of ['HIDE_CMP', 'OPEN_OPTIONS', 'HIDE_CMP', 'DO_CONSENT', 'SAVE_CONSENT']) {
            if (cmp[step]) await act(cmp[step].action, cmp);
          }
          if (clicks > 0) post('shoji:consent:done');
        }
      } catch (x) {
        // Left as the site showed it, for the user to answer.
        hidden.forEach(function (el) { el.classList.remove('shoji-cmp-hidden'); });
      }
      hidden = [];
      busy = false;
      // Another notice may follow (a consent platform drawn in steps, or a second one).
      stopAt = Math.max(stopAt, Date.now() + 2000);
      setTimeout(scan, 500);
    },
  };

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})(__CONSENT_DETECTORS__);
