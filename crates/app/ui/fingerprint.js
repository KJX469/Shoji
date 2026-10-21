// Fingerprinting protection: small, stable noise per site and session in what pages read back
// from canvas, WebGL and audio, and fewer details about the hardware. Injected at document start
// in every frame, before the page's own scripts. A site sees the same values throughout a session
// and different ones from other sites, so they can't be used to follow you between them.
(function (F) {
  'use strict';
  if (window.__shojiFp) return;
  Object.defineProperty(window, '__shojiFp', { value: true });
  // Captcha and verification frames compare these values against known devices; leave them alone.
  var EXEMPT = ['challenges.cloudflare.com', 'hcaptcha.com', 'recaptcha.net', 'arkoselabs.com', 'funcaptcha.com'];
  var h = location.hostname;
  for (var i = 0; i < EXEMPT.length; i++) {
    var e = EXEMPT[i];
    if (h === e || h.slice(-e.length - 1) === '.' + e) return;
  }
  if (h === 'www.google.com' && location.pathname.indexOf('/recaptcha/') === 0) return;

  /* ---- seeded generator: the same stream for the same site and session ---- */
  function rng(salt) {
    var s = (F.seed ^ salt) >>> 0;
    return function () {
      s = (s + 0x6D2B79F5) >>> 0;
      var t = s;
      t = Math.imul(t ^ (t >>> 15), t | 1);
      t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  /* ---- wrap without changing what the function looks like ---- */
  // A proxy keeps the name and length; toString is answered with the original's source.
  var originals = new WeakMap();
  var nativeToString = Function.prototype.toString;
  var toStringProxy = new Proxy(nativeToString, {
    apply: function (target, self, args) {
      return Reflect.apply(target, originals.get(self) || self, args);
    },
  });
  originals.set(toStringProxy, nativeToString);
  try { Function.prototype.toString = toStringProxy; } catch (x) { /* frozen */ }
  function disguise(orig, handler) {
    var p = new Proxy(orig, handler);
    originals.set(p, orig);
    return p;
  }
  function wrap(obj, name, make) {
    try {
      var orig = obj[name];
      if (typeof orig !== 'function') return;
      var impl = make(orig);
      obj[name] = disguise(orig, { apply: function (target, self, args) { return impl.call(self, target, args); } });
    } catch (x) { /* read-only in this frame */ }
  }
  function getter(obj, name, value) {
    try {
      var d = Object.getOwnPropertyDescriptor(obj, name);
      if (!d || !d.get) return;
      var g = disguise(d.get, { apply: function (t, self) { return value(Reflect.apply(t, self, [])); } });
      Object.defineProperty(obj, name, { get: g, set: d.set, enumerable: d.enumerable, configurable: true });
    } catch (x) { /* not here */ }
  }

  /* ---- canvas: flip the low bit of a few drawn pixels, chosen by the seed ---- */
  // Only opaque pixels: a change to a transparent one is lost when the canvas is exported, and
  // text drawn on a clear background leaves most pixels transparent.
  function noisePixels(data, w, h) {
    if (!data || data.length < 4) return;
    var r = rng(w * 7919 + h);
    var count = data.length >> 2;
    var n = Math.max(1, Math.min(32, count >> 9));
    for (var i = 0; i < n; i++) {
      var start = Math.floor(r() * count), channel = Math.floor(r() * 3);
      for (var k = 0; k < count; k++) {
        var p = ((start + k) % count) << 2;
        if (data[p + 3] === 255) { data[p + channel] ^= 1; break; }
      }
    }
  }
  var C2D = window.CanvasRenderingContext2D && CanvasRenderingContext2D.prototype;
  var origGetImageData = C2D && C2D.getImageData;
  if (C2D) {
    wrap(C2D, 'getImageData', function (orig) {
      return function (target, args) {
        var img = Reflect.apply(target, this, args);
        noisePixels(img.data, img.width, img.height);
        return img;
      };
    });
  }
  // Exports go through a copy, so the page's own canvas is never changed.
  function noisyCopy(canvas) {
    var w = canvas.width, h = canvas.height;
    if (!w || !h || w * h > 4096 * 4096) return null;
    var copy = document.createElement('canvas');
    copy.width = w; copy.height = h;
    var ctx = copy.getContext('2d');
    if (!ctx) return null;
    ctx.drawImage(canvas, 0, 0);
    var img = origGetImageData.call(ctx, 0, 0, w, h);
    noisePixels(img.data, w, h);
    ctx.putImageData(img, 0, 0);
    return copy;
  }
  var CV = window.HTMLCanvasElement && HTMLCanvasElement.prototype;
  if (CV) {
    ['toDataURL', 'toBlob'].forEach(function (name) {
      wrap(CV, name, function () {
        return function (target, args) {
          var copy = null;
          try { copy = noisyCopy(this); } catch (x) { copy = null; }
          return Reflect.apply(target, copy || this, args);
        };
      });
    });
  }

  /* ---- WebGL: the same pixel noise for readPixels; Strict hides the GPU's name ---- */
  [window.WebGLRenderingContext, window.WebGL2RenderingContext].forEach(function (GL) {
    if (!GL) return;
    var proto = GL.prototype;
    wrap(proto, 'readPixels', function () {
      return function (target, args) {
        var out = Reflect.apply(target, this, args);
        var buf = args[6];
        if (buf && buf.length) noisePixels(buf, args[2] | 0, args[3] | 0);
        return out;
      };
    });
    if (F.strict) {
      wrap(proto, 'getParameter', function () {
        return function (target, args) {
          var p = args[0];
          if (p === 0x9245) return 'Google Inc.';        // UNMASKED_VENDOR_WEBGL
          if (p === 0x9246) return 'ANGLE (Generic GPU)'; // UNMASKED_RENDERER_WEBGL
          return Reflect.apply(target, this, args);
        };
      });
    }
  });

  /* ---- audio: a tiny, stable change to rendered samples ---- */
  var AB = window.AudioBuffer && AudioBuffer.prototype;
  if (AB) {
    var touched = new WeakSet();
    var noiseAudio = function (data) {
      var r = rng(data.length);
      for (var i = 0; i < data.length; i += 97) data[i] *= 1 + (r() - 0.5) * 1e-6;
    };
    wrap(AB, 'getChannelData', function () {
      return function (target, args) {
        var data = Reflect.apply(target, this, args);
        if (!touched.has(data)) { touched.add(data); noiseAudio(data); }
        return data;
      };
    });
    wrap(AB, 'copyFromChannel', function () {
      return function (target, args) {
        var out = Reflect.apply(target, this, args);
        if (args[0] && args[0].length) noiseAudio(args[0]);
        return out;
      };
    });
  }
  var AN = window.AnalyserNode && AnalyserNode.prototype;
  if (AN) {
    wrap(AN, 'getFloatFrequencyData', function () {
      return function (target, args) {
        var out = Reflect.apply(target, this, args);
        var a = args[0], r = rng(a ? a.length : 0);
        if (a) for (var i = 0; i < a.length; i += 31) a[i] += (r() - 0.5) * 1e-4;
        return out;
      };
    });
  }

  /* ---- hardware: a believable smaller core count; Strict also caps reported memory ---- */
  var NP = window.Navigator && Navigator.prototype;
  if (NP) {
    getter(NP, 'hardwareConcurrency', function (real) {
      if (!(real > 2)) return real;
      return 2 + Math.floor(rng(0x4c50)() * (real - 1));
    });
    if (F.strict) {
      getter(NP, 'deviceMemory', function (real) { return real > 4 ? 4 : real; });
    }
  }
})(__SHOJI_FP__);
