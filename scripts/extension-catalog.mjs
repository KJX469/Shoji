// Builds the extension store's catalog: the most-installed Manifest V3 extensions of each
// Chrome Web Store category, with their icons embedded so the store page makes no requests of its
// own. Usage: node scripts/extension-catalog.mjs out.json
import { writeFileSync } from 'node:fs';

const UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36';
const BASE = 'https://chromewebstore.google.com';

const CATEGORIES = {
  'make_chrome_yours/privacy': 'Privacy & security',
  'make_chrome_yours/accessibility': 'Accessibility',
  'make_chrome_yours/functionality': 'Customization',
  'productivity/tools': 'Tools',
  'productivity/workflow': 'Workflow',
  'productivity/developer': 'Developer tools',
  'productivity/communication': 'Communication',
  'productivity/education': 'Education',
  'lifestyle/shopping': 'Shopping',
  'lifestyle/entertainment': 'Entertainment',
  'lifestyle/social': 'Social',
  'lifestyle/news': 'News & weather',
  'lifestyle/travel': 'Travel',
  'lifestyle/well_being': 'Well-being',
  'lifestyle/art': 'Art & design',
  'lifestyle/household': 'Home',
  'lifestyle/games': 'Games',
  'lifestyle/fun': 'Fun',
};

const get = async (url, tries = 3) => {
  for (let i = 1; ; i++) {
    try {
      const r = await fetch(url, { headers: { 'user-agent': UA }, signal: AbortSignal.timeout(30000) });
      if (!r.ok) throw new Error(`${r.status}`);
      return r;
    } catch (e) {
      if (i >= tries) throw new Error(`${url}: ${e.message}`);
      await new Promise(r => setTimeout(r, 1000 * i));
    }
  }
};

/** The listing data the page embeds (the `ds:1` block), as JSON. */
const embedded = html => {
  const at = html.indexOf("AF_initDataCallback({key: 'ds:1'");
  if (at < 0) return null;
  const from = html.indexOf('data:', at) + 5;
  const to = html.indexOf(', sideChannel', from);
  return to > from ? JSON.parse(html.slice(from, to)) : null;
};

/** Every extension record in `node`: [id, icon, name, rating, ratings, _, summary, ...]. */
const records = (node, out = []) => {
  if (!Array.isArray(node)) return out;
  if (typeof node[0] === 'string' && /^[a-p]{32}$/.test(node[0]) && typeof node[2] === 'string' && typeof node[3] === 'number') {
    out.push(node);
    return out;
  }
  for (const n of node) records(n, out);
  return out;
};

const manifestOf = r => {
  const raw = r.find(v => typeof v === 'string' && v.includes('"manifest_version"'));
  try { return raw ? JSON.parse(raw) : null; } catch { return null; }
};

const found = new Map();
for (const [slug, label] of Object.entries(CATEGORIES)) {
  const html = await (await get(`${BASE}/category/extensions/${slug}`)).text();
  const data = embedded(html);
  const list = data ? records(data) : [];
  let kept = 0;
  for (const r of list) {
    const manifest = manifestOf(r);
    // The engine only runs Manifest V3.
    if (!manifest || manifest.manifest_version !== 3 || found.has(r[0])) continue;
    const users = r.slice(8).find(v => typeof v === 'number' && v > 1) ?? 0;
    found.set(r[0], {
      id: r[0],
      name: r[2].trim(),
      cat: label,
      text: typeof r[6] === 'string' ? r[6].trim() : '',
      rating: Math.round(r[3] * 10) / 10,
      ratings: typeof r[4] === 'number' ? r[4] : 0,
      users,
      icon: r[1],
    });
    kept++;
  }
  console.log(`${slug}: ${list.length} listed, ${kept} new Manifest V3`);
}

// Icons are fetched here and embedded, so opening the store contacts no one.
const items = [...found.values()];
let next = 0;
await Promise.all(Array.from({ length: 8 }, async () => {
  while (next < items.length) {
    const it = items[next++];
    try {
      const r = await get(`${it.icon}=s64-rw`);
      const type = r.headers.get('content-type') || 'image/webp';
      it.icon = `data:${type};base64,${Buffer.from(await r.arrayBuffer()).toString('base64')}`;
    } catch (e) {
      console.warn(`no icon for ${it.id}: ${e.message}`);
      it.icon = '';
    }
  }
}));

items.sort((a, b) => b.users - a.users);
if (items.length < 200) throw new Error(`only ${items.length} extensions found; the page layout probably changed`);
writeFileSync(process.argv[2], JSON.stringify(items));
console.log(`${items.length} extensions`);
