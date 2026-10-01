#!/usr/bin/env node
/**
 * Writes docs/releases/data/releases.json — the last-resort data source for the
 * download page when api.github.com is unreachable or rate-limited.
 *
 * The snapshot is a faithful copy of the API response, not a derived structure:
 * it keeps GitHub's snake_case field names and each asset's original `name` and
 * `browser_download_url`. That is what lets the page run the live response and
 * the snapshot through the same classifier (docs/releases/assets/releases.js),
 * so the two paths cannot drift apart.
 *
 * Run it after publishing a release: `npm run snapshot:releases`.
 */
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const OWNER = process.env.CPUM_RELEASES_OWNER ?? 'open-nexa';
const REPO = process.env.CPUM_RELEASES_REPO ?? 'cpum';
const API_URL = `https://api.github.com/repos/${OWNER}/${REPO}/releases?per_page=30`;

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const outPath = join(root, 'docs', 'releases', 'data', 'releases.json');

const response = await fetch(API_URL, {
  headers: { Accept: 'application/vnd.github+json' },
});

if (!response.ok) {
  console.error(`[snapshot:releases] API returned ${response.status} — snapshot left untouched`);
  process.exit(1);
}

const releases = await response.json();

if (!Array.isArray(releases)) {
  console.error('[snapshot:releases] API did not return a list — snapshot left untouched');
  process.exit(1);
}

const snapshot = releases
  .filter((release) => !release.draft)
  .map((release) => ({
    tag_name: release.tag_name,
    name: release.name,
    html_url: release.html_url,
    published_at: release.published_at,
    prerelease: Boolean(release.prerelease),
    draft: Boolean(release.draft),
    assets: (release.assets || []).map((asset) => ({
      name: asset.name,
      browser_download_url: asset.browser_download_url,
      size: asset.size,
    })),
  }));

mkdirSync(dirname(outPath), { recursive: true });
writeFileSync(outPath, `${JSON.stringify(snapshot, null, 2)}\n`, 'utf8');

const assetCount = snapshot.reduce((total, release) => total + release.assets.length, 0);
console.log(`[snapshot:releases] ${snapshot.length} releases, ${assetCount} assets -> ${outPath}`);
