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

  /* ---- tell the shield which surfaces the page read (once each, for its counter) ---- */
  var post = window.chrome && chrome.webview && chrome.webview.postMessage.bind(chrome.webview);
  var reported = {};
  function seen(what) {
    if (reported[what] || !post) return;
    reported[what] = true;
    try { post('byobu:fp:' + what); } catch (x) { /* detached */ }
  }

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
        seen('canvas');
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
          seen('canvas');
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
        seen('webgl');
        if (buf && buf.length) noisePixels(buf, args[2] | 0, args[3] | 0);
        return out;
      };
    });
    if (F.strict) {
      wrap(proto, 'getParameter', function () {
        return function (target, args) {
          var p = args[0];
          if (p === 0x9245 || p === 0x9246) seen('gpu');
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
        seen('audio');
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
      seen('hardware');
      if (!(real > 2)) return real;
      return 2 + Math.floor(rng(0x4c50)() * (real - 1));
    });
    if (F.strict) {
      getter(NP, 'deviceMemory', function (real) { return real > 4 ? 4 : real; });
    }
    // Battery level and charging time change slowly enough to follow a device between sites.
    try { delete NP.getBattery; } catch (x) { /* fixed */ }
  }

  /* ---- media devices: no names until the page may use a camera or microphone ---- */
  var MD = window.MediaDevices && MediaDevices.prototype;
  if (MD && navigator.permissions) {
    var allowed = function (name) {
      return navigator.permissions.query({ name: name }).then(function (p) { return p.state === 'granted'; }, function () { return false; });
    };
    wrap(MD, 'enumerateDevices', function () {
      return function (target, args) {
        return Promise.all([Reflect.apply(target, this, args), allowed('camera'), allowed('microphone')]).then(function (r) {
          var list = r[0], cam = r[1], mic = r[2];
          return list.map(function (d) {
            var ok = d.kind === 'videoinput' ? cam : mic;
            if (ok || !d.label) return d;
            seen('devices');
            var plain = { deviceId: d.deviceId, kind: d.kind, label: '', groupId: d.groupId };
            plain.toJSON = function () { return { deviceId: d.deviceId, kind: d.kind, label: '', groupId: d.groupId }; };
            return plain;
          });
        });
      };
    });
  }

  /* ---- speech voices: only the system's default, not every installed language pack ---- */
  var SS = window.SpeechSynthesis && SpeechSynthesis.prototype;
  if (SS) {
    wrap(SS, 'getVoices', function () {
      return function (target, args) {
        var all = Reflect.apply(target, this, args);
        seen('voices');
        var def = all.filter(function (v) { return v.default; });
        return def.length ? def : all.slice(0, 1);
      };
    });
  }

  if (!F.strict) return;

  /* ---- Strict: screen and window sizes rounded to common ones ---- */
  var SIZES = [[1366, 768], [1440, 900], [1536, 864], [1600, 900], [1920, 1080], [2560, 1440], [3840, 2160]];
  function common(w, h) {
    for (var i = 0; i < SIZES.length; i++) if (SIZES[i][0] >= w && SIZES[i][1] >= h) return SIZES[i];
    return SIZES[SIZES.length - 1];
  }
  var SP = window.Screen && Screen.prototype;
  if (SP) {
    var size = function () { seen('screen'); return common(window.innerWidth, window.innerHeight); };
    getter(SP, 'width', function () { return size()[0]; });
    getter(SP, 'height', function () { return size()[1]; });
    getter(SP, 'availWidth', function () { return size()[0]; });
    getter(SP, 'availHeight', function () { return size()[1] - 48; });
    getter(SP, 'availLeft', function () { return 0; });
    getter(SP, 'availTop', function () { return 0; });
  }
  // The window's frame and position say which toolbars and monitor layout this is.
  var WP = window;
  ['outerWidth', 'outerHeight', 'screenX', 'screenY', 'screenLeft', 'screenTop'].forEach(function (name) {
    try {
      var d = Object.getOwnPropertyDescriptor(WP, name);
      if (!d || !d.get) return;
      var inner = name === 'outerWidth' ? 'innerWidth' : name === 'outerHeight' ? 'innerHeight' : null;
      var g = disguise(d.get, { apply: function () { return inner ? window[inner] : 0; } });
      Object.defineProperty(WP, name, { get: g, set: d.set, enumerable: d.enumerable, configurable: true });
    } catch (x) { /* fixed */ }
  });

  /* ---- Strict: font checks answered from a fixed set that every Windows PC has ---- */
  var BASE_FONTS = ['arial', 'arial black', 'calibri', 'cambria', 'candara', 'comic sans ms', 'consolas',
    'constantia', 'corbel', 'courier new', 'georgia', 'impact', 'lucida console', 'segoe ui',
    'segoe ui emoji', 'segoe ui symbol', 'tahoma', 'times new roman', 'trebuchet ms', 'verdana',
    'serif', 'sans-serif', 'monospace', 'cursive', 'fantasy', 'system-ui'];
  var FS = document.fonts && Object.getPrototypeOf(document.fonts);
  if (FS) {
    wrap(FS, 'check', function () {
      return function (target, args) {
        var spec = String(args[0] || '');
        var family = spec.replace(/^.*?\d+(\.\d+)?(px|pt|em|rem|%)(\/\S+)?\s+/, '').split(',')[0].trim().replace(/^["']|["']$/g, '').toLowerCase();
        if (BASE_FONTS.indexOf(family) >= 0) return Reflect.apply(target, this, args);
        // A web font the page loaded itself is answered truthfully.
        var own = false;
        try { this.forEach(function (f) { if (f.family.replace(/^["']|["']$/g, '').toLowerCase() === family) own = true; }); } catch (x) { /* none */ }
        if (own) return Reflect.apply(target, this, args);
        seen('fonts');
        return false;
      };
    });
  }
  try { delete window.queryLocalFonts; delete Window.prototype.queryLocalFonts; } catch (x) { /* fixed */ }

  /* ---- Strict: the browser's detailed version and hardware answered generically ---- */
  var UAD = window.NavigatorUAData && NavigatorUAData.prototype;
  if (UAD) {
    wrap(UAD, 'getHighEntropyValues', function () {
      return function (target, args) {
        var self = this;
        seen('client-hints');
        return Reflect.apply(target, this, args).then(function (v) {
          var major = (self.brands || []).map(function (b) { return { brand: b.brand, version: b.version + '.0.0.0' }; });
          if ('architecture' in v) v.architecture = 'x86';
          if ('bitness' in v) v.bitness = '64';
          if ('model' in v) v.model = '';
          if ('platformVersion' in v) v.platformVersion = '10.0.0';
          if ('fullVersionList' in v) v.fullVersionList = major;
          if ('uaFullVersion' in v) {
            var b = major.filter(function (x) { return /Chrom/.test(x.brand); })[0];
            v.uaFullVersion = b ? b.version : v.uaFullVersion;
          }
          if ('formFactors' in v) v.formFactors = ['Desktop'];
          return v;
        });
      };
    });
  }
})(__SHOJI_FP__);
