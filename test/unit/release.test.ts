import assert from 'node:assert';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// `bun test` runs this file with its own `expect` and `test` in place of vitest's.
import { expect, test } from 'vitest';

const rootDir = path.resolve(import.meta.dirname, '..', '..');
const { plugins } = JSON.parse(fs.readFileSync(path.join(rootDir, '.releaserc.json'), 'utf8')) as {
  plugins: (string | [string, Record<string, string>])[];
};
const releasePlugin = plugins.find(
  (plugin): plugin is [string, { pkgRoot: string }] =>
    Array.isArray(plugin) && plugin[0] === './script/releasePlugin.mjs'
);
assert.ok(releasePlugin);
// The files script/release.mjs reads or runs, copied so that a regressed dry run builds nothing in this checkout.
const releaseFiles = ['script', '.releaserc.json', path.join(releasePlugin[1].pkgRoot, 'package.json')];
// A draft target standing for the commit of the repository a run releases from.
const headCommit = 'HEAD';
const olderCommit = 'a'.repeat(40);
const pendingMarker = '\n\n<!-- pending release -->';

// Serves GitHub releases and registry versions from RELEASE_TEST_STATE, and appends every request to RELEASE_TEST_LOG.
// The first request matching each of RELEASE_TEST_STATE's `failures` (a method and part of the URL) fails in the given way.
const fakeApi = `
import fs from 'node:fs';
const { drafts, npmCommits, failures } = JSON.parse(process.env.RELEASE_TEST_STATE);
const deleted = new Set();
const refs = new Map();
globalThis.fetch = async (url, init = {}) => {
  const method = init.method ?? 'GET';
  fs.appendFileSync(process.env.RELEASE_TEST_LOG, JSON.stringify({ tool: 'fetch', method, url }) + '\\n');
  const key = Object.keys(failures).find((key) => key.startsWith(method + ' ') && url.includes(key.split(' ')[1]));
  const failure = failures[key];
  delete failures[key];
  if (failure === 'drop') throw new TypeError('fetch failed');
  if (failure === 'rateLimit') return new Response('', { status: 429, headers: { 'retry-after': '1' } });
  if (failure === 'serverError') return new Response('', { status: 502 });
  if (failure === 'validationError') return Response.json({ message: 'Validation Failed' }, { status: 422 });
  const response = respond(method, url, init);
  if (failure === 'dropAfterProcessing') throw new TypeError('fetch failed');
  return response;
};
function respond(method, url, init) {
  if (url.endsWith('/releases?per_page=100')) return Response.json(drafts);
  if (url.startsWith('https://registry.npmjs.org/')) {
    const commit = npmCommits[url.split('/').at(-1)];
    return commit ? Response.json({ gitHead: commit }) : new Response('', { status: 404 });
  }
  if (url.startsWith('https://crates.io/')) return new Response('', { status: 404 });
  if (url.endsWith('/git/refs') && method === 'POST') {
    const { ref, sha } = JSON.parse(init.body);
    if (refs.has(ref)) return Response.json({ message: 'Reference already exists' }, { status: 422 });
    refs.set(ref, sha);
  }
  if (url.includes('/git/ref/')) {
    const sha = refs.get('refs/' + url.split('/git/ref/')[1]);
    return sha ? Response.json({ object: { sha } }) : Response.json({ message: 'Not Found' }, { status: 404 });
  }
  if (method === 'DELETE') {
    if (deleted.has(url)) {
      return url.includes('/git/refs/')
        ? Response.json({ message: 'Reference does not exist' }, { status: 422 })
        : new Response('', { status: 404 });
    }
    deleted.add(url);
  }
  return method === 'GET' ? Response.json({}) : new Response(null, { status: 204 });
}
`;

const fakeWb = `#!/bin/sh
printf '{"tool":"wb","args":"%s"}\\n' "$*" >> "$RELEASE_TEST_LOG"
`;

interface Draft {
  id: number;
  tag_name: string;
  target_commitish: string;
}

interface Request {
  tool: 'fetch' | 'wb';
  method?: string;
  url?: string;
  args?: string;
}

type Failure = 'drop' | 'dropAfterProcessing' | 'rateLimit' | 'serverError' | 'validationError';

function runRelease(
  args: string[],
  refName: string,
  drafts: Draft[],
  npmCommits: Record<string, string> = {},
  inCi = true,
  failures: Record<string, Failure> = {}
): { status: number | null; output: string; requests: Request[] } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-test-'));
  try {
    const repoDir = path.join(dir, 'repo');
    for (const file of releaseFiles) fs.cpSync(path.join(rootDir, file), path.join(repoDir, file), { recursive: true });
    const git = (...gitArgs: string[]): string =>
      spawnSync('git', gitArgs, { cwd: repoDir, encoding: 'utf8' }).stdout.trim();
    git('init', '--quiet');
    git(
      '-c',
      'user.name=test',
      '-c',
      'user.email=test@example.com',
      'commit',
      '--quiet',
      '--allow-empty',
      '-m',
      'test'
    );
    const head = git('rev-parse', 'HEAD');
    fs.writeFileSync(path.join(dir, 'fakeApi.mjs'), fakeApi);
    fs.writeFileSync(path.join(dir, 'wb'), fakeWb, { mode: 0o755 });
    const logPath = path.join(dir, 'requests.jsonl');
    fs.writeFileSync(logPath, '');
    const result = spawnSync('node', ['--import', path.join(dir, 'fakeApi.mjs'), 'script/release.mjs', ...args], {
      cwd: repoDir,
      encoding: 'utf8',
      env: {
        ...process.env,
        CI: inCi ? 'true' : '',
        PATH: `${dir}${path.delimiter}${process.env.PATH}`,
        GITHUB_REF_NAME: refName,
        GITHUB_REPOSITORY: 'WillBooster/tree-sitter',
        GITHUB_TOKEN: 'fake',
        RELEASE_TEST_LOG: logPath,
        RELEASE_TEST_STATE: JSON.stringify({
          drafts: drafts.map((draft) => ({
            ...draft,
            target_commitish: draft.target_commitish === headCommit ? head : draft.target_commitish,
            draft: true,
            body: `notes${pendingMarker}`,
          })),
          npmCommits,
          failures,
        }),
      },
    });
    const requests = fs
      .readFileSync(logPath, 'utf8')
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line) as Request);
    return { status: result.status, output: result.stdout + result.stderr, requests };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const writesOf = (requests: Request[]): Request[] =>
  requests.filter((request) => request.tool === 'wb' || request.method !== 'GET');
const formatRequest = ({ method, url }: Request): string =>
  `${method} ${url?.replace(/^.*\/repos\/[^/]+\/[^/]+\//, '')}`;

// GitHub lists the newest release first: v1.0.2 is held by npm, and v1.0.1 by no registry.
const olderDrafts: Draft[] = [
  { id: 2, tag_name: 'v1.0.2', target_commitish: olderCommit },
  { id: 1, tag_name: 'v1.0.1', target_commitish: olderCommit },
];
const olderNpmCommits = { '1.0.2': olderCommit };

for (const [args, inCi] of [
  [[], true],
  [['--', '--no-ci'], false],
] as const) {
  test(`a real run ${inCi ? 'in CI' : 'outside CI with --no-ci'} deletes an unheld draft and dispatches the pending release`, () => {
    const { status, requests } = runRelease([...args], 'main', olderDrafts, olderNpmCommits, inCi);

    expect(status).toBe(0);
    expect(writesOf(requests).map(formatRequest)).toEqual([
      'DELETE releases/1',
      'POST git/refs',
      'POST actions/workflows/release.yml/dispatches',
    ]);
  });
}

test('a real run retries transient failures of GitHub and the registries', () => {
  const { status, requests } = runRelease([], 'main', olderDrafts, olderNpmCommits, true, {
    'GET /releases?per_page=100': 'drop',
    'GET https://registry.npmjs.org/': 'serverError',
    'DELETE /releases/1': 'dropAfterProcessing',
    'POST /git/refs': 'dropAfterProcessing',
    'POST /actions/workflows/release.yml/dispatches': 'rateLimit',
  });

  expect(status).toBe(0);
  expect(writesOf(requests).map(formatRequest)).toEqual([
    'DELETE releases/1',
    'DELETE releases/1',
    'POST git/refs',
    'POST actions/workflows/release.yml/dispatches',
    'POST actions/workflows/release.yml/dispatches',
  ]);
  // Beyond the default timeout, since each retry waits a second.
}, 30_000);

test('a real run on a pending-release branch retries deleting the branch after a dropped connection', () => {
  const { status, requests } = runRelease([], 'release-pending/v1.0.2', [], {}, true, {
    'DELETE /git/refs/heads/release-pending/v1.0.2': 'dropAfterProcessing',
  });

  expect(status).toBe(0);
  expect(writesOf(requests).map(formatRequest)).toEqual([
    'POST actions/workflows/release.yml/dispatches',
    'DELETE git/refs/heads/release-pending/v1.0.2',
    'DELETE git/refs/heads/release-pending/v1.0.2',
  ]);
});

test('a run outside CI without --no-ci reports the deferral without writes', () => {
  const { status, output, requests } = runRelease([], 'main', olderDrafts, olderNpmCommits, false);

  expect(status).toBe(0);
  expect(output).toContain('Would delete the draft release v1.0.1');
  expect(output).toContain('Would dispatch a run on release-pending/v1.0.2');
  expect(writesOf(requests)).toEqual([]);
});

for (const args of [['--dry-run'], ['--dry'], ['-d'], ['--', '--dry-run'], ['--', '-d']]) {
  test(`a dry run with ${args.join(' ')} reports the deferral without writes`, () => {
    const { status, output, requests } = runRelease(args, 'main', olderDrafts, olderNpmCommits);

    expect(status).toBe(0);
    expect(output).toContain('Would delete the draft release v1.0.1');
    expect(output).toContain('Would dispatch a run on release-pending/v1.0.2');
    expect(writesOf(requests)).toEqual([]);
  });
}

test('a dry run on a pending-release branch reports completing the release without writes', () => {
  const { status, output, requests } = runRelease(['--dry-run'], 'release-pending/v1.0.2', [
    { id: 3, tag_name: 'v1.0.2', target_commitish: headCommit },
  ]);

  expect(status).toBe(0);
  expect(output).toContain('Would build and publish the pending release v1.0.2');
  expect(output).toContain('Would dispatch a run on main and delete the branch release-pending/v1.0.2');
  expect(writesOf(requests)).toEqual([]);
});

for (const args of [
  ['--', '--dry-run', '--debug'],
  ['--', '--debug'],
]) {
  test(`a run with ${args.join(' ')} and no pending release forwards its arguments to the release`, () => {
    const { status, requests } = runRelease(args, 'main', []);

    expect(status).toBe(0);
    expect(writesOf(requests)).toEqual([{ tool: 'wb', args: `release ${args.join(' ')}` }]);
  });
}

for (const args of [
  ['--dry-run=true'],
  ['--d'],
  ['-vd'],
  ['--', '-d', '--no-d'],
  ['--', '--dry'],
  ['--debug'],
  ['--no-ci'],
  ['--', '--ci=false'],
]) {
  test(`${args.join(' ')} is refused before any request`, () => {
    const { status, output, requests } = runRelease(args, 'main', olderDrafts, olderNpmCommits);

    expect(status).not.toBe(0);
    expect(output).toContain('Unsupported argument');
    expect(requests).toEqual([]);
  });
}

for (const failure of ['drop', 'serverError'] as const) {
  test(`a real run fails instead of repeating a dispatch that GitHub may have processed (${failure})`, () => {
    const { status, requests } = runRelease([], 'main', olderDrafts, olderNpmCommits, true, {
      'POST /actions/workflows/release.yml/dispatches': failure,
    });

    expect(status).not.toBe(0);
    expect(writesOf(requests).map(formatRequest)).toEqual([
      'DELETE releases/1',
      'POST git/refs',
      'POST actions/workflows/release.yml/dispatches',
    ]);
  });
}

test('a real run reports why creating the pending-release branch failed', () => {
  const { status, output } = runRelease([], 'main', olderDrafts, olderNpmCommits, true, {
    'POST /git/refs': 'validationError',
  });

  expect(status).not.toBe(0);
  expect(output).toContain('POST git/refs failed: 422');
});
