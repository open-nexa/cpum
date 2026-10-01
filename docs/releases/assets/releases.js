// Release data for the CPU Manager download page.
//
// The rule this file exists to enforce: a download link is never *built* from a
// name template. A template has to guess the version segment, the separator and
// the archive suffix, and it guesses wrong the moment the release pipeline
// renames something. Instead every asset keeps the `name` and
// `browser_download_url` that the GitHub API returned, and the only work done
// here is *classification*: which of these files is an installer, for which
// architecture, and which is a checksum manifest.
//
// Three sources, in order: live API -> localStorage cache (stale is fine) ->
// the committed snapshot under data/releases.json. The API allows anonymous
// cross-origin reads but only 60 requests per hour per IP, so a visitor who
// navigates twice must not spend two requests.

const OWNER = 'open-nexa';
const REPO = 'cpum';
const API_URL = `https://api.github.com/repos/${OWNER}/${REPO}/releases?per_page=30`;
const SNAPSHOT_URL = 'data/releases.json';
const CACHE_KEY = 'cpum-releases-v1';
const CACHE_TTL_MS = 15 * 60 * 1000;

// Deliberately narrow: an architecture spelling that is not listed here is
// dropped rather than guessed at. A missing download is a bug somebody can see
// and fix; a mislabelled one silently hands a user the wrong binary.
const ARCH_MAP = {
  x64: 'x86_64',
  amd64: 'x86_64',
  x86_64: 'x86_64',
  arm64: 'aarch64',
  aarch64: 'aarch64',
};

// The pipeline publishes two installers per architecture:
//
//   CPU-Manager_v0.1.0_windows-x64-setup.exe   canonical, renamed by release.yml
//   CPU.Manager_0.1.0_x64-setup.exe            Tauri's own NSIS output, kept
//                                              because latest.json points at it
//
// The canonical one is what a human should download. The Tauri-named one stays
// published for the built-in updater, but is classified separately so the page
// never shows two buttons for the same architecture.
const CANONICAL_INSTALLER = /^CPU-Manager_.+?_windows-(x64|arm64)-setup\.exe$/i;
const TAURI_INSTALLER = /^CPU[ .]Manager_.+?_(x64|arm64|aarch64)-setup\.exe$/i;

export function classifyAsset(name) {
  if (/\.sig$/i.test(name)) return null;

  if (name === 'SHA256SUMS.txt') return { kind: 'checksums' };
  if (name === 'latest.json') return { kind: 'manifest' };

  // Checked before the generic NSIS pattern: `-setup.exe` must win over the
  // updater's `.nsis.zip` so an installer is never labelled as a payload.
  const canonical = CANONICAL_INSTALLER.exec(name);
  if (canonical) {
    return { kind: 'installer', arch: ARCH_MAP[canonical[1].toLowerCase()], variant: 'canonical' };
  }

  const tauri = TAURI_INSTALLER.exec(name);
  if (tauri) {
    return { kind: 'installer', arch: ARCH_MAP[tauri[1].toLowerCase()], variant: 'updater' };
  }

  if (/\.nsis\.zip$/i.test(name)) {
    const arch = /(x64|amd64|arm64|aarch64)/i.exec(name);
    return {
      kind: 'installer',
      arch: arch ? ARCH_MAP[arch[1].toLowerCase()] : null,
      variant: 'updater',
    };
  }

  return null;
}

const ARCH_ORDER = { x86_64: 0, aarch64: 1 };

// One installer row per architecture. If the canonical file is missing for an
// architecture, fall back to the Tauri-named one rather than showing nothing -
// a broken pipeline should degrade to "still downloadable", not to an empty
// page.
export function pickInstallers(assets) {
  const rows = new Map();
  for (const asset of assets) {
    const info = classifyAsset(asset.name);
    if (!info || info.kind !== 'installer' || !info.arch) continue;
    const current = rows.get(info.arch);
    if (!current) {
      rows.set(info.arch, { asset, variant: info.variant });
    } else if (current.variant === 'updater' && info.variant === 'canonical') {
      rows.set(info.arch, { asset, variant: 'canonical' });
    }
  }
  return [...rows.entries()]
    .sort((a, b) => (ARCH_ORDER[a[0]] ?? 99) - (ARCH_ORDER[b[0]] ?? 99))
    .map(([arch, row]) => ({ arch, asset: row.asset }));
}

export function findKind(assets, kind) {
  for (const asset of assets) {
    const info = classifyAsset(asset.name);
    if (info && info.kind === kind) return asset;
  }
  return null;
}

export function formatSize(bytes) {
  if (typeof bytes !== 'number' || !Number.isFinite(bytes)) return '';
  const mb = bytes / (1024 * 1024);
  if (mb < 1) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${mb.toFixed(1)} MB`;
}

export function archLabel(arch) {
  if (arch === 'x86_64') return '64-bit (x64)';
  if (arch === 'aarch64') return 'ARM64';
  return arch;
}

function readCache() {
  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (!parsed || !Array.isArray(parsed.releases)) return null;
    return parsed;
  } catch {
    return null;
  }
}

function writeCache(releases) {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify({ fetchedAt: Date.now(), releases }));
  } catch {
    // A full or disabled storage is not worth failing the page over.
  }
}

// One request shared by every caller on the page, so two sections rendering at
// once do not spend two of the hourly quota.
let inflight = null;

async function fetchLive() {
  const response = await fetch(API_URL, {
    headers: { Accept: 'application/vnd.github+json' },
  });
  if (!response.ok) throw new Error(`releases API returned ${response.status}`);
  const releases = await response.json();
  if (!Array.isArray(releases)) throw new Error('releases API did not return a list');
  return releases.map((release) => ({
    tag_name: release.tag_name,
    name: release.name,
    html_url: release.html_url,
    published_at: release.published_at,
    prerelease: Boolean(release.prerelease),
    draft: Boolean(release.draft),
    body: release.body || '',
    assets: (release.assets || []).map((asset) => ({
      name: asset.name,
      browser_download_url: asset.browser_download_url,
      size: asset.size,
    })),
  }));
}

async function fetchSnapshot() {
  const response = await fetch(SNAPSHOT_URL, { cache: 'no-cache' });
  if (!response.ok) throw new Error(`snapshot returned ${response.status}`);
  const data = await response.json();
  if (!Array.isArray(data)) throw new Error('snapshot did not contain a list');
  return data;
}

export function loadReleases() {
  if (inflight) return inflight;

  inflight = (async () => {
    try {
      const releases = await fetchLive();
      writeCache(releases);
      return { releases, source: 'live' };
    } catch (error) {
      const cached = readCache();
      if (cached) {
        return { releases: cached.releases, source: 'cache', stale: Date.now() - cached.fetchedAt > CACHE_TTL_MS };
      }
      try {
        const releases = await fetchSnapshot();
        return { releases, source: 'snapshot', error };
      } catch (snapshotError) {
        return { releases: [], source: 'failed', error: snapshotError };
      }
    }
  })();

  return inflight;
}
