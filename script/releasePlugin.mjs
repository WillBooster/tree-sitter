// A semantic-release plugin that publishes the crate, the npm package, and the GitHub Release in the `prepare` step.
// semantic-release pushes the version tag between `prepare` and `publish`, so a failure in `publish` would leave the
// tag behind, and a re-run would find no new commits and never complete the release. Here the tag is created last, by
// publishing a draft GitHub Release that records the version, the commit, and the notes before any registry receives
// the version. A re-run of a failed release computes the same version and skips each registry that already holds it
// from the same commit; script/release.mjs completes a release left pending when a newer commit reaches the branch.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

// crates.io rejects requests with a generic User-Agent.
const userAgent = 'willbooster-release (https://github.com/WillBooster)';
// Marks the drafts this release flow creates, so that it never completes or deletes a draft someone else prepared.
const pendingMarker = '\n\n<!-- pending release -->';

export function verifyConditions(pluginConfig, { env }) {
  // `crate` is optional, for a repository that publishes only the npm package.
  if (!pluginConfig.pkgRoot) throw new Error('Set the `pkgRoot` option.');
  for (const name of ['GITHUB_REPOSITORY', 'GITHUB_TOKEN']) {
    if (!env[name]) throw new Error(`${name} is not set.`);
  }
}

export async function prepare(pluginConfig, { cwd, env, logger, nextRelease }) {
  const { gitHead, gitTag, name, notes, version } = nextRelease;
  const github = createGitHubClient(env);
  let draft = await findDraftRelease(github, gitTag);
  if (draft && draft.target_commitish !== gitHead) {
    throw new Error(`The draft release ${gitTag} targets ${draft.target_commitish}, not ${gitHead}.`);
  }
  draft ??= await github('POST', 'releases', {
    tag_name: gitTag,
    target_commitish: gitHead,
    name,
    body: `${notes}${pendingMarker}`,
    draft: true,
  });
  await publishRelease({ ...pluginConfig, cwd, env, logger, draft, version });
}

/** Publishes the version to every registry that does not hold it yet, and then publishes the draft release. */
export async function publishRelease({ crate, pkgRoot, cwd, env, logger, draft, version }) {
  const gitHead = draft.target_commitish;
  const pkgDir = path.resolve(cwd, pkgRoot);
  const run = (command, args, dir) => execFileSync(command, args, { cwd: dir, env, stdio: 'inherit' });
  const publishedCommits = await fetchPublishedCommits({ crate, cwd, pkgRoot, version });
  const targets = publishedCommits.map((target) => ({
    ...target,
    ...(target.registry === 'crates.io'
      ? {
          dryRun: () => run('cargo', ['publish', '--dry-run', '--allow-dirty', '-p', crate], cwd),
          publish: () => run(path.join(cwd, 'script', 'publish-crate'), [crate], cwd),
        }
      : {
          dryRun: () => run('npm', ['publish', '--dry-run'], pkgDir),
          publish: () => run('npm', ['publish'], pkgDir),
        }),
  }));
  for (const { commit, name } of targets) {
    if (commit !== undefined && commit !== gitHead) {
      throw new Error(`${name} was published from ${commit || 'an unknown commit'}, not from ${gitHead}.`);
    }
  }

  const unpublished = targets.filter(({ commit }) => commit === undefined);
  // Dry runs catch packaging errors before any registry receives the version.
  for (const target of unpublished) target.dryRun();
  for (const target of targets) {
    if (unpublished.includes(target)) target.publish();
    else logger.log(`Skipped ${target.name}, which is already published from ${gitHead}`);
  }

  // Publishing the draft creates the tag, so semantic-release's tag push that follows changes nothing.
  const release = await createGitHubClient(env)('PATCH', `releases/${draft.id}`, {
    draft: false,
    body: draft.body.slice(0, -pendingMarker.length),
  });
  logger.log(`Published the GitHub Release ${release.html_url}`);
}

/**
 * Returns the commit each registry published the version from: `undefined` for an unpublished version, and an empty
 * string when the registry records no commit.
 */
export async function fetchPublishedCommits({ crate, cwd, pkgRoot, version }) {
  const { name: pkgName } = JSON.parse(fs.readFileSync(path.resolve(cwd, pkgRoot, 'package.json'), 'utf8'));
  return [
    ...(crate
      ? [
          {
            registry: 'crates.io',
            name: `${crate}@${version} on crates.io`,
            commit: await fetchPublishedCommit(
              `https://crates.io/api/v1/crates/${crate}/${version}`,
              (body) => body.version.trustpub_data?.sha
            ),
          },
        ]
      : []),
    {
      registry: 'npm',
      name: `${pkgName}@${version} on npm`,
      commit: await fetchPublishedCommit(
        `https://registry.npmjs.org/${pkgName.replace('/', '%2f')}/${version}`,
        (body) => body.gitHead
      ),
    },
  ];
}

async function fetchPublishedCommit(url, getCommit) {
  const response = await fetch(url, { headers: { 'User-Agent': userAgent } });
  if (response.status === 404) return;
  if (!response.ok) throw new Error(`GET ${url} failed: ${response.status} ${await response.text()}`);
  return getCommit(await response.json()) ?? '';
}

/** Returns the draft releases that this release flow created, which GitHub lists before the published releases. */
export async function listPendingReleases(github) {
  const releases = await github('GET', 'releases?per_page=100');
  return releases.filter((release) => release.draft && release.body?.endsWith(pendingMarker));
}

export async function findDraftRelease(github, gitTag) {
  const drafts = await listPendingReleases(github);
  return drafts.find((release) => release.tag_name === gitTag);
}

export function createGitHubClient(env) {
  return async (method, route, body) => {
    const response = await fetch(`https://api.github.com/repos/${env.GITHUB_REPOSITORY}/${route}`, {
      method,
      headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${env.GITHUB_TOKEN}` },
      ...(body && { body: JSON.stringify(body) }),
    });
    if (!response.ok) throw new Error(`${method} ${route} failed: ${response.status} ${await response.text()}`);
    return response.status === 204 ? undefined : response.json();
  };
}
