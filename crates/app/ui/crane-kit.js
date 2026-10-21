/* Shared crane behaviour (see crane-kit.css). Exposes window.Pet. */
(() => {
  const calm = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const LOOPING = new Set(['guard', 'carry', 'sleep']);
  const timers = new WeakMap();

  const Pet = {
    calm,
    /** Play a pose on a crane. One-shot poses settle back to idle on their own. */
    pose(el, name) {
      if (!el) return;
      clearTimeout(timers.get(el));
      el.removeAttribute('data-pose');
      if (!name) return;
      void el.getBoundingClientRect(); // restart the animation if the same pose comes round again
      el.setAttribute('data-pose', name);
      if (!LOOPING.has(name)) {
        timers.set(el, setTimeout(() => el.removeAttribute('data-pose'), 2200));
      }
    },
    /** A flock crosses the page, wings beating, then is gone. */
    flock(count = 7, source = document.querySelector('.crane')) {
      if (calm || !source || !document.body.animate) return;
      const w = innerWidth, h = innerHeight;
      for (let i = 0; i < count; i++) {
        const c = source.cloneNode(true);
        c.removeAttribute('data-pose');
        c.classList.remove('live');
        const size = 26 + Math.random() * 30;
        const fromLeft = Math.random() < .5;
        c.style.cssText = `position:fixed;z-index:2147483000;pointer-events:none;width:${size}px;height:${size}px;`
          + `left:0;top:0;filter:var(--crane-shadow);`;
        document.body.appendChild(c);
        const y0 = h * (.15 + Math.random() * .6), y1 = y0 - h * (.1 + Math.random() * .3);
        const x0 = fromLeft ? -80 : w + 80, x1 = fromLeft ? w + 80 : -80;
        const flip = fromLeft ? 1 : -1;
        const dur = 3200 + Math.random() * 2200, delay = i * 160 + Math.random() * 240;
        const run = c.animate([
          { transform: `translate(${x0}px, ${y0}px) scale(${flip}, 1) rotate(${-8 * flip}deg)` },
          { transform: `translate(${(x0 + x1) / 2}px, ${(y0 + y1) / 2 - 24}px) scale(${flip}, 1) rotate(${-14 * flip}deg)`, offset: .5 },
          { transform: `translate(${x1}px, ${y1}px) scale(${flip}, 1) rotate(${-8 * flip}deg)` },
        ], { duration: dur, delay, easing: 'ease-in-out', fill: 'both' });
        c.querySelectorAll('path:nth-of-type(2), path:nth-of-type(3)').forEach(p => {
          p.style.transformBox = 'fill-box';
          p.style.transformOrigin = '0% 55%';
          p.animate([{ transform: 'scaleX(1.05)' }, { transform: 'scaleX(.3)' }],
            { duration: 260, delay, iterations: Infinity, direction: 'alternate', easing: 'ease-in-out' });
        });
        run.onfinish = () => c.remove();
      }
    },
  };
  /** After `ms` without input the page rests: looping crane animations pause (crane-kit.css),
      `onRest` runs, and the returned function reports true. Any input wakes it and `onWake` runs.
      A page nobody is looking at then draws no frames (the idle crane alone kept WebView2 busy). */
  Pet.rest = (ms = 65000, onRest, onWake) => {
    let timer = 0, resting = false;
    const settle = () => { resting = true; document.body.classList.add('resting'); if (onRest) onRest(); };
    const wake = () => {
      clearTimeout(timer);
      if (resting) { resting = false; document.body.classList.remove('resting'); if (onWake) onWake(); }
      timer = setTimeout(settle, ms);
    };
    ['mousemove', 'keydown', 'pointerdown', 'wheel', 'focus'].forEach(ev => addEventListener(ev, wake, { passive: true }));
    document.addEventListener('visibilitychange', wake);
    wake();
    return () => resting;
  };
  window.Pet = Pet;
})();
