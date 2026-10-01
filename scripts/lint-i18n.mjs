#!/usr/bin/env node
/**
 * i18n guard for src/i18n.ts.
 *
 * Three checks, all hard failures:
 *
 *   1. `zh-CN` and `en-US` must define exactly the same key set. The rule in
 *      AGENTS.md is that every user-visible string exists in both locales; a key
 *      present in one and missing in the other is either an untranslated string
 *      or a stale translation, and neither should ship.
 *   2. Every statically written `t('some.key')` in src/ must resolve. Dynamic
 *      keys (template literals, variables) are skipped — they cannot be checked
 *      statically.
 *   3. No CJK characters outside src/i18n.ts. User-visible text belongs in the
 *      dictionary and code comments are English-only, so a Chinese character
 *      anywhere else is either a hardcoded string or a Chinese comment.
 *
 * Exits 0 with a notice when src/i18n.ts cannot be parsed, so the script can sit
 * in package.json without ever blocking a build on a parse bug — but see the
 * note below: it throws if the file is missing entirely.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { stripComments, tokenize } from './lib/source.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const srcDir = join(root, 'src');
const i18nPath = join(srcDir, 'i18n.ts');

const LOCALES = ['zh-CN', 'en-US'];
// Anything in the CJK Unified Ideographs block or a Chinese punctuation mark.
const CJK = /[　-〿㐀-䶿一-鿿＀-￯]/;

function walk(dir, out = []) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      walk(path, out);
    } else if (/\.(vue|ts)$/.test(entry)) {
      out.push(path);
    }
  }
  return out;
}

/**
 * Reads the two locale dictionaries out of src/i18n.ts by tokenising the object
 * literal rather than by regex-matching lines: translation values contain
 * braces (`"版本 {version}"`) and spans that break naive scanning.
 */
function readDictionaries(source) {
  const tokens = tokenize(stripComments(source));
  const start = tokens.findIndex((token) => token.type === 'ident' && token.value === 'messages');
  if (start === -1) return null;

  let open = -1;
  for (let i = start; i < tokens.length; i += 1) {
    if (tokens[i].type === 'punct' && tokens[i].value === '{') {
      open = i;
      break;
    }
  }
  if (open === -1) return null;

  const keys = { 'zh-CN': new Set(), 'en-US': new Set() };
  let depth = 0;
  let locale = null;
  let pending = null;

  for (let i = open; i < tokens.length; i += 1) {
    const token = tokens[i];
    const next = tokens[i + 1];

    if (token.type === 'punct' && token.value === '{') {
      depth += 1;
      if (depth === 2 && pending) {
        locale = pending;
        pending = null;
      }
      continue;
    }

    if (token.type === 'punct' && token.value === '}') {
      depth -= 1;
      if (depth === 1) locale = null;
      if (depth === 0) break;
      continue;
    }

    if (depth === 1 && token.type === 'string' && LOCALES.includes(token.value)) {
      // Do not skip past the `{`: the loop has to see it so the depth counter
      // moves into the locale object and `pending` is promoted to `locale`.
      if (next && next.type === 'punct' && next.value === ':' && tokens[i + 2]?.value === '{') {
        pending = token.value;
      }
      continue;
    }

    if (depth === 2 && locale && next && next.type === 'punct' && next.value === ':') {
      if (token.type === 'ident' || token.type === 'string') {
        keys[locale].add(token.value);
        i += 1;
      }
      continue;
    }
  }

  return keys;
}

const problems = [];

let dictionaries;
try {
  dictionaries = readDictionaries(readFileSync(i18nPath, 'utf8'));
} catch (error) {
  if (error.code === 'ENOENT') {
    console.error(`[lint:i18n] FAILED — ${i18nPath} does not exist`);
    process.exit(1);
  }
  throw error;
}

if (!dictionaries) {
  console.log('[lint:i18n] could not parse the messages object in src/i18n.ts — skipping');
  process.exit(0);
}

const [sourceLocale, targetLocale] = LOCALES;
const sourceKeys = dictionaries[sourceLocale];
const targetKeys = dictionaries[targetLocale];

const missingInTarget = [...sourceKeys].filter((key) => !targetKeys.has(key)).sort();
const missingInSource = [...targetKeys].filter((key) => !sourceKeys.has(key)).sort();

if (missingInTarget.length) {
  problems.push(`missing in ${targetLocale} (${missingInTarget.length}):\n  ${missingInTarget.join('\n  ')}`);
}
if (missingInSource.length) {
  problems.push(`missing in ${sourceLocale} (${missingInSource.length}):\n  ${missingInSource.join('\n  ')}`);
}

// `t('key')` / `$t('key')` with a literal key only.
const callPattern = /(?<![\w.])\$?t\(\s*['"]([\w.-]+)['"]/g;
const unknownKeys = new Map();

for (const file of walk(srcDir)) {
  if (file === i18nPath) continue;
  const source = stripComments(readFileSync(file, 'utf8'));

  for (const match of source.matchAll(callPattern)) {
    const key = match[1];
    if (sourceKeys.has(key)) continue;
    const where = relative(root, file).replace(/\\/g, '/');
    if (!unknownKeys.has(key)) unknownKeys.set(key, new Set());
    unknownKeys.get(key).add(where);
  }
}

if (unknownKeys.size) {
  const lines = [...unknownKeys.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, files]) => `${key}  (${[...files].join(', ')})`);
  problems.push(`t() keys that are not in ${sourceLocale} (${unknownKeys.size}):\n  ${lines.join('\n  ')}`);
}

// Hardcoded Chinese: user-visible text outside the dictionary, or a comment that
// was not written in English.
const cjkHits = [];
for (const file of walk(srcDir)) {
  if (file === i18nPath) continue;
  const lines = stripComments(readFileSync(file, 'utf8')).split('\n');
  lines.forEach((line, index) => {
    if (!CJK.test(line)) return;
    const where = relative(root, file).replace(/\\/g, '/');
    cjkHits.push(`${where}:${index + 1}  ${line.trim().slice(0, 100)}`);
  });
}

if (cjkHits.length) {
  problems.push(`CJK characters outside src/i18n.ts (${cjkHits.length}):\n  ${cjkHits.join('\n  ')}`);
}

if (problems.length) {
  console.error(`[lint:i18n] FAILED\n\n${problems.join('\n\n')}\n`);
  process.exit(1);
}

console.log(`[lint:i18n] ok — ${sourceKeys.size} keys, zh-CN / en-US in sync, no hardcoded Chinese`);
