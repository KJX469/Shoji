// Media with sound waits for the person: until they click or press a key in this page, audible
// audio and video don't start on their own (muted video still does). The engine's own autoplay
// policy can't do this in WebView2, which counts every navigation as user activity. Injected at
// document start in every frame.
(function () {
  'use strict';
  if (window.__shojiAutoplay) return;
  Object.defineProperty(window, '__shojiAutoplay', { value: true });
  var M = window.HTMLMediaElement && HTMLMediaElement.prototype;
  if (!M) return;
  var allowed = false;
  function grant(e) {
    if (e.isTrusted) allowed = true;
  }
  ['pointerdown', 'keydown', 'touchstart'].forEach(function (t) {
    addEventListener(t, grant, { capture: true, passive: true });
  });
  function audible(m) {
    return !m.muted && m.volume > 0;
  }
  // Keep `play` looking native (name, length, toString), as the fingerprinting code does.
  var play = M.play;
  var wrapped = new Proxy(play, {
    apply: function (target, self, args) {
      if (!allowed && self instanceof HTMLMediaElement && audible(self)) {
        return Promise.reject(new DOMException("play() failed because the user didn't interact with the document first.", 'NotAllowedError'));
      }
      return Reflect.apply(target, self, args);
    },
  });
  try {
    Object.defineProperty(M, 'play', { value: wrapped, writable: true, configurable: true });
    var toString = Function.prototype.toString;
    Function.prototype.toString = new Proxy(toString, {
      apply: function (target, self, args) {
        return Reflect.apply(target, self === wrapped ? play : self, args);
      },
    });
  } catch (e) { /* leave it */ }
  // The `autoplay` attribute, and media unmuted before any interaction, are stopped as they start.
  function stop(e) {
    var m = e.target;
    if (!allowed && m instanceof HTMLMediaElement && audible(m) && !m.paused) {
      try { m.pause(); } catch (x) { /* detached */ }
    }
  }
  document.addEventListener('play', stop, true);
  document.addEventListener('volumechange', stop, true);
})();
