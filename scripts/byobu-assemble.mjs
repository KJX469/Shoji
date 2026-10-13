// Assembles uBlock Origin's scriptlets and redirect resources into the JSON that Brave's
// `adblock` engine consumes (an array of `Resource`s).
//
//   node scripts/byobu-assemble.mjs <out.json> [work-dir]
//
// uBO keeps its scriptlets as ES modules; importing them in Node and calling `toString()` on
// the registered functions is exactly what uBO itself does when it injects them.
// Keeps a shallow sparse clone in <work-dir> so repeat runs only fetch what changed.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { extname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const out = resolve(process.argv[2] ?? 'ubo-resources.json');
const work = resolve(process.argv[3] ?? join(process.env.TEMP ?? '/tmp', 'byobu-ubo'));
const repo = join(work, 'uBlock');
const git = (...a) => execFileSync('git', a, { stdio: 'pipe', cwd: existsSync(repo) ? repo : work });

mkdirSync(work, { recursive: true });
if (existsSync(join(repo, '.git'))) {
  git('fetch', '--depth', '1', 'origin', 'master');
  git('reset', '--hard', 'origin/master');
} else {
  git('clone', '--depth', '1', '--filter=blob:none', '--sparse', 'https://github.com/gorhill/uBlock.git', repo);
  execFileSync(
    'git',
    ['sparse-checkout', 'set', '--no-cone', '/src/js/resources/', '/src/js/*.js', '/src/web_accessible_resources/'],
    { cwd: repo },
  );
}
const head = execFileSync('git', ['rev-parse', '--short', 'HEAD'], { cwd: repo }).toString().trim();

const imp = (p) => import(pathToFileURL(join(repo, p)).href);
const { builtinScriptlets } = await imp('src/js/resources/scriptlets.js');
const { default: redirects } = await imp('src/js/redirect-resources.js');

const b64 = (b) => Buffer.from(b).toString('base64');
const resources = [];
const seen = new Set();

// Scriptlets: functions are used as-is; `*.fn` entries are helpers other scriptlets depend on.
for (const s of builtinScriptlets) {
  if (seen.has(s.name)) continue;
  seen.add(s.name);
  const isHelper = s.name.endsWith('.fn');
  resources.push({
    name: s.name,
    aliases: s.aliases ?? [],
    kind: { mime: isHelper ? 'fn/javascript' : 'application/javascript' },
    content: b64(s.fn.toString()),
    dependencies: (s.dependencies ?? []).filter((d) => typeof d === 'string'),
    // Anything needing trust is injectable only from trusted lists (bit 0), as in Brave.
    ...(s.requiresTrust ? { permission: 1 } : {}),
  });
}

// Redirect stubs from web_accessible_resources, as listed in redirect-resources.js.
const MIME = {
  '.gif': 'image/gif', '.png': 'image/png', '.js': 'application/javascript', '.html': 'text/html',
  '.mp4': 'video/mp4', '.mp3': 'audio/mp3', '.json': 'application/json', '.txt': 'text/plain',
  '.xml': 'text/xml', '.css': 'text/css',
};
const dir = join(repo, 'src/web_accessible_resources');
let stubs = 0;
for (const [name, details] of redirects) {
  const file = join(dir, name);
  if (!existsSync(file)) continue;
  const mime = name === 'empty' ? 'text/plain' : (MIME[extname(name)] ?? 'application/octet-stream');
  const aliases = [details.alias].flat().filter(Boolean);
  if (seen.has(name)) continue;
  seen.add(name);
  resources.push({
    name,
    aliases,
    kind: { mime },
    content: b64(readFileSync(file)),
  });
  stubs++;
}

writeFileSync(out, JSON.stringify(resources));
console.log(`uBlock ${head}: ${resources.length - stubs} scriptlets/helpers, ${stubs} redirect stubs -> ${out}`);
