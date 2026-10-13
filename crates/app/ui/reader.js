/* Shoji reader mode: pull the article out of the current page.
 *
 * Same idea as Firefox's Readability: score paragraph-like blocks by how much real prose they
 * hold (commas, length), credit their ancestors, discount link-heavy and boilerplate-named
 * containers, take the best-scoring node, then rebuild it from a small whitelist of tags so
 * nothing from the page (scripts, styles, handlers, forms) survives into the reader view.
 * Returns {title, byline, site, html} or null if no article-like content was found. */
(() => {
  const NEG = /(comment|meta|footer|footnote|reflist|references|navbox|catlinks|editsection|mw-empty|infobox|hatnote|toc|sidebar|sponsor|promo|banner|share|social|related|recommend|widget|menu|nav|breadcrumb|cookie|popup|modal|newsletter|subscribe|advert|\bads?\b|outbrain|taboola)/i;
  const POS = /(article|body|content|entry|main|page|post|story|text|blog|hentry|prose)/i;
  const SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'NOSCRIPT', 'IFRAME', 'OBJECT', 'EMBED', 'FORM', 'BUTTON', 'INPUT', 'SELECT', 'TEXTAREA', 'SVG', 'CANVAS', 'NAV', 'ASIDE', 'FOOTER', 'HEADER', 'DIALOG', 'TEMPLATE', 'AUDIO', 'VIDEO']);
  const KEEP = new Set(['P', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'PRE', 'CODE', 'EM', 'STRONG', 'B', 'I', 'A', 'IMG', 'FIGURE', 'FIGCAPTION', 'BR', 'HR', 'TABLE', 'THEAD', 'TBODY', 'TR', 'TH', 'TD', 'SUP', 'SUB']);
  const text = el => (el.textContent || '').replace(/\s+/g, ' ').trim();
  const idClass = el => (el.id || '') + ' ' + (typeof el.className === 'string' ? el.className : '');
  const linkDensity = el => {
    const total = text(el).length;
    if (!total) return 0;
    let links = 0;
    el.querySelectorAll('a').forEach(a => { links += text(a).length; });
    return links / total;
  };
  const hidden = el => {
    if (el.hidden || el.getAttribute('aria-hidden') === 'true') return true;
    const s = el.style;
    return !!s && (s.display === 'none' || s.visibility === 'hidden');
  };
  const inBoilerplate = el => {
    for (let n = el; n && n !== document.body; n = n.parentElement) {
      if (SKIP_TAGS.has(n.tagName) || hidden(n)) return true;
      const role = n.getAttribute && n.getAttribute('role');
      if (role === 'navigation' || role === 'banner' || role === 'complementary' || role === 'contentinfo') return true;
    }
    return false;
  };

  const scores = new Map();
  const bump = (el, amount) => {
    if (!el || el === document.documentElement) return;
    if (!scores.has(el)) {
      let init = 0;
      const ic = idClass(el);
      if (NEG.test(ic)) init -= 25;
      if (POS.test(ic)) init += 25;
      if (el.tagName === 'ARTICLE') init += 15;
      scores.set(el, init);
    }
    scores.set(el, scores.get(el) + amount);
  };

  const body = document.body;
  if (!body) return null;
  body.querySelectorAll('p, pre, td').forEach(el => {
    const t = text(el);
    if (t.length < 25 || inBoilerplate(el)) return;
    const score = 1 + (t.split(',').length - 1) + Math.min(Math.floor(t.length / 100), 3);
    bump(el.parentElement, score);
    bump(el.parentElement && el.parentElement.parentElement, score / 2);
  });

  let top = null, best = 0;
  scores.forEach((raw, el) => {
    const s = raw * (1 - linkDensity(el));
    if (s > best) { best = s; top = el; }
  });
  if (!top) top = body.querySelector('article, main, [role=main]');
  if (!top || text(top).length < 200) return null;

  const abs = (u, bases) => {
    try {
      const url = new URL(u, document.baseURI);
      return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : null;
    } catch { return null; }
  };
  const out = document.createElement('div');
  let words = 0;
  const walk = (src, dst) => {
    for (const node of src.childNodes) {
      if (node.nodeType === 3) {
        if (node.nodeValue) dst.appendChild(document.createTextNode(node.nodeValue));
        words += node.nodeValue ? node.nodeValue.trim().split(/\s+/).length : 0;
        continue;
      }
      if (node.nodeType !== 1) continue;
      const tag = node.tagName;
      if (SKIP_TAGS.has(tag) || hidden(node)) continue;
      const ic = idClass(node);
      if (NEG.test(ic) && !POS.test(ic) && node !== top) continue;
      // Link-heavy blocks with little prose are menus and "more stories" lists.
      if (['DIV', 'SECTION', 'UL', 'OL', 'TABLE'].includes(tag) && text(node).length < 400 && linkDensity(node) > 0.5) continue;
      if (tag === 'IMG') {
        const raw = node.currentSrc || node.getAttribute('src') || node.getAttribute('data-src') || node.getAttribute('data-original');
        const href = raw && abs(raw);
        if (href && (node.naturalWidth === 0 || node.naturalWidth > 80 || !node.complete)) {
          const img = document.createElement('img');
          img.setAttribute('src', href);
          img.setAttribute('alt', node.getAttribute('alt') || '');
          img.setAttribute('loading', 'lazy');
          dst.appendChild(img);
        }
        continue;
      }
      if (KEEP.has(tag)) {
        const el = document.createElement(tag.toLowerCase());
        if (tag === 'A') {
          const href = abs(node.getAttribute('href') || '');
          if (href) { el.setAttribute('href', href); el.setAttribute('rel', 'noreferrer noopener'); }
        }
        if ((tag === 'TD' || tag === 'TH') && node.colSpan > 1) el.setAttribute('colspan', String(Math.min(node.colSpan, 20)));
        walk(node, el);
        // Drop empty wrappers (but keep rules, breaks and images).
        if (['BR', 'HR'].includes(tag) || el.querySelector('img') || el.textContent.trim()) dst.appendChild(el);
      } else {
        walk(node, dst); // unknown container: keep its content, lose the wrapper
      }
    }
  };
  walk(top, out);
  if (words < 40) return null;

  const meta = n => { const m = document.querySelector(`meta[property="${n}"], meta[name="${n}"]`); return m && m.content ? m.content.trim() : ''; };
  const h1 = top.querySelector('h1') || document.querySelector('h1');
  const byEl = document.querySelector('[rel=author], .byline, .author, [itemprop=author]');
  return {
    title: (meta('og:title') || (h1 && text(h1)) || document.title || '').slice(0, 300),
    byline: (meta('author') || meta('article:author') || (byEl && text(byEl)) || '').slice(0, 120),
    site: (meta('og:site_name') || location.hostname.replace(/^www\./, '')).slice(0, 80),
    html: out.innerHTML.slice(0, 2000000),
  };
})()
