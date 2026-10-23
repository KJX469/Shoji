// Every page's base protections, registered once per tab before its first page, so no page can
// load without them: Global Privacy Control, De-AMP, holding back sound until the person
// interacts, and fingerprinting noise. Choices come in as __SHOJI_BASE__ (the switches, and the
// sites where each protection is off); site-specific cosmetics and scriptlets come separately.
// Runs in every frame; frames take their top page's site.
(function (C) {
  'use strict';
  if (window.__shojiBase) return;
  Object.defineProperty(window, '__shojiBase', { value: true });
  var top = location.hostname, a = location.ancestorOrigins;
  if (a && a.length) {
    try { top = new URL(a[a.length - 1]).hostname; } catch (e) { /* opaque */ }
  }
  top = (top || '').toLowerCase();
  if (!top || top.indexOf('shoji.') === 0) return; // Shoji's own pages
  var bare = top.replace(/^www\./, '');
  function covers(list) {
    for (var i = 0; i < (list || []).length; i++) {
      var r = String(list[i]).toLowerCase().replace(/^\*\./, '').replace(/^www\./, '');
      if (r && (bare === r || bare.slice(-r.length - 1) === '.' + r)) return true;
    }
    return false;
  }

  /* ---- the site (registrable domain), for the fingerprinting seed ---- */
  // Suffixes under which each name is its own site: country second levels (co.uk, com.au, ...)
  // and hosting platforms that give every customer a subdomain.
  var SECOND = { co: 1, com: 1, net: 1, org: 1, gov: 1, edu: 1, ac: 1, or: 1, ne: 1, go: 1, gob: 1,
    mil: 1, nic: 1, ltd: 1, plc: 1, sch: 1, nhs: 1, nom: 1, gv: 1, gen: 1, biz: 1, info: 1 };
  var HOSTED = ['github.io', 'gitlab.io', 'blogspot.com', 'herokuapp.com', 'vercel.app', 'netlify.app',
    'pages.dev', 'workers.dev', 'web.app', 'firebaseapp.com', 'azurewebsites.net', 'cloudfront.net',
    'appspot.com', 'wordpress.com', 'tumblr.com', 'substack.com', 'neocities.org', 'glitch.me',
    'fly.dev', 'onrender.com', 'repl.co', 's3.amazonaws.com'];
  function site(h) {
    if (/^[\d.]+$/.test(h) || h.indexOf(':') >= 0) return h; // an address
    var l = h.split('.'), n = l.length;
    if (n <= 2) return h;
    for (var i = 0; i < HOSTED.length; i++) {
      var s = HOSTED[i];
      if (h === s) return h;
      if (h.slice(-s.length - 1) === '.' + s) return l.slice(n - s.split('.').length - 1).join('.');
    }
    if (l[n - 1].length === 2 && SECOND[l[n - 2]]) return l.slice(n - 3).join('.');
    return l.slice(n - 2).join('.');
  }
  function hash(s) {
    var h = 0x811c9dc5;
    for (var i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193); }
    return h >>> 0;
  }

  if (C.gpc) {
    __SHOJI_GPC__
  }
  if (C.deamp && !covers(C.shieldOff)) {
    __SHOJI_DEAMP__
  }
  if (C.autoplay && !covers(C.autoplaySites)) {
    __SHOJI_AUTOPLAY__
  }
  var fp = covers(C.fpOff) ? 'off' : C.fp;
  if (fp === 'standard' || fp === 'strict') {
    var __fp = { seed: hash(C.key + '|' + site(top)), strict: fp === 'strict' };
    __SHOJI_FINGERPRINT__
  }
})(__SHOJI_BASE__);
