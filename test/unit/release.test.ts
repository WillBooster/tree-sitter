import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { expect, test } from 'bun:test';

const rootDir = path.resolve(import.meta.dirname, '..', '..');
// The files script/release.mjs reads or runs, copied so that a regressed dry run builds nothing in this checkout.
const releaseFiles = ['script', '.releaserc.json', 'lib/binding_web/package.json'];
// A draft target standing for the commit of the repository a run releases from.
const headCommit = 'HEAD';
const olderCommit = 'a'.repeat(40);
const pendingMarker = '\n\n<!-- pending release -->';

// Serves GitHub releases and registry versions from RELEASE_TEST_STATE, and appends every request to RELEASE_TEST_LOG.
const fakeApi = `
import fs from 'node:fs';
const { drafts, npmCommits } = JSON.parse(process.env.RELEASE_TEST_STATE);
globalThis.fetch = async (url, init = {}) => {
  const method = init.method ?? 'GET';
  fs.appendFileSync(process.env.RELEASE_TEST_LOG, JSON.stringify({ tool: 'fetch', method, url }) + '\\n');
  if (url.endsWith('/releases?per_page=100')) return Response.json(drafts);
  if (url.startsWith('https://registry.npmjs.org/')) {
    const commit = npmCommits[url.split('/').at(-1)];
    return commit ? Response.json({ gitHead: commit }) : new Response('', { status: 404 });
  }
  if (url.startsWith('https://crates.io/')) return new Response('', { status: 404 });
  return method === 'GET' ? Response.json({}) : new Response(null, { status: 204 });
};
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

function runRelease(args: string[], refName: string, drafts: Draft[], npmCommits: Record<string, string> = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'release-test-'));
  try {
    const repoDir = path.join(dir, 'repo');
    for (const file of releaseFiles) fs.cpSync(path.join(rootDir, file), path.join(repoDir, file), { recursive: true });
    const git = (...gitArgs: string[]) =>
      spawnSync('git', gitArgs, { cwd: repoDir, encoding: 'utf8' }).stdout.trim();
    git('init', '--quiet');
    git('-c', 'user.name=test', '-c', 'user.email=test@example.com', 'commit', '--quiet', '--allow-empty', '-m', 'test');
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

const writesOf = (requests: Request[]) =>
  requests.filter((request) => request.tool === 'wb' || request.method !== 'GET');

// GitHub lists the newest release first: v1.0.2 is held by npm, and v1.0.1 by no registry.
const olderDrafts: Draft[] = [
  { id: 2, tag_name: 'v1.0.2', target_commitish: olderCommit },
  { id: 1, tag_name: 'v1.0.1', target_commitish: olderCommit },
];
const olderNpmCommits = { '1.0.2': olderCommit };

test('a real run deletes an unheld draft and dispatches the pending release', () => {
  const { status, requests } = runRelease([], 'main', olderDrafts, olderNpmCommits);

  expect(status).toBe(0);
  expect(writesOf(requests).map(({ method, url }) => `${method} ${url?.replace(/^.*\/repos\/[^/]+\/[^/]+\//, '')}`)).toEqual([
    'DELETE releases/1',
    'POST git/refs',
    'POST actions/workflows/release.yml/dispatches',
  ]);
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

test('a run without pending releases forwards its arguments to the release', () => {
  for (const args of [['--', '--dry-run', '--debug'], ['--', '--debug']]) {
    const { status, requests } = runRelease(args, 'main', []);

    expect(status).toBe(0);
    expect(writesOf(requests)).toEqual([{ tool: 'wb', args: `release ${args.join(' ')}` }]);
  }
});

for (const args of [['--dry-run=true'], ['--d'], ['-vd'], ['--', '-d', '--no-d'], ['--', '--dry'], ['--debug']]) {
  test(`${args.join(' ')} is refused before any request`, () => {
    const { status, output, requests } = runRelease(args, 'main', olderDrafts, olderNpmCommits);

    expect(status).not.toBe(0);
    expect(output).toContain('Unsupported argument');
    expect(requests).toEqual([]);
  });
}
