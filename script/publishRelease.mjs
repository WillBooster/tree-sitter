// A semantic-release plugin that publishes the crate, the npm package, and the GitHub Release in the `prepare` step.
// semantic-release pushes the version tag between `prepare` and `publish`, so a failure in `publish` leaves the tag
// behind, and a re-run finds no new commits and never completes the release. Here the GitHub Release creates the tag
// last, and a re-run of a failed release computes the same version and skips each registry that already holds it
// from the same commit.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

// crates.io rejects requests with a generic User-Agent.
const userAgent = 'willbooster-release (https://github.com/WillBooster)';

export function verifyConditions(pluginConfig, { env }) {
  if (!pluginConfig.crate || !pluginConfig.pkgRoot) throw new Error('Set the `crate` and `pkgRoot` options.');
  for (const name of ['GITHUB_REPOSITORY', 'GITHUB_TOKEN']) {
    if (!env[name]) throw new Error(`${name} is not set.`);
  }
}

export async function prepare({ crate, pkgRoot }, { cwd, env, logger, nextRelease }) {
  const { gitHead, gitTag, name, notes, version } = nextRelease;
  const pkgDir = path.resolve(cwd, pkgRoot);
  const { name: pkgName } = JSON.parse(fs.readFileSync(path.join(pkgDir, 'package.json'), 'utf8'));
  const run = (command, args, dir) => execFileSync(command, args, { cwd: dir, env, stdio: 'inherit' });
  const targets = [
    {
      name: `${crate}@${version} on crates.io`,
      commit: await fetchPublishedCommit(
        `https://crates.io/api/v1/crates/${crate}/${version}`,
        (body) => body.version.trustpub_data?.sha
      ),
      dryRun: () => run('cargo', ['publish', '--dry-run', '--allow-dirty', '-p', crate], cwd),
      publish: () => run(path.join(cwd, 'script', 'publish-crate'), [crate], cwd),
    },
    {
      name: `${pkgName}@${version} on npm`,
      commit: await fetchPublishedCommit(
        `https://registry.npmjs.org/${pkgName.replace('/', '%2f')}/${version}`,
        (body) => body.gitHead
      ),
      dryRun: () => run('npm', ['publish', '--dry-run'], pkgDir),
      publish: () => run('npm', ['publish'], pkgDir),
    },
  ];
  for (const { commit, name } of targets) {
    if (commit !== undefined && commit !== gitHead) {
      throw new Error(
        `${name} was published from ${commit || 'an unknown commit'}, not from ${gitHead}. ` +
          'Complete the release of that commit before releasing this one.'
      );
    }
  }

  const unpublished = targets.filter(({ commit }) => commit === undefined);
  // Dry runs catch packaging errors before any registry receives the version.
  for (const target of unpublished) target.dryRun();
  for (const target of targets) {
    if (unpublished.includes(target)) target.publish();
    else logger.log(`Skipped ${target.name}, which is already published from ${gitHead}`);
  }

  // Creating the release also creates the tag, so semantic-release's tag push that follows changes nothing.
  const response = await fetch(`https://api.github.com/repos/${env.GITHUB_REPOSITORY}/releases`, {
    method: 'POST',
    headers: { Accept: 'application/vnd.github+json', Authorization: `Bearer ${env.GITHUB_TOKEN}` },
    body: JSON.stringify({ tag_name: gitTag, target_commitish: gitHead, name, body: notes }),
  });
  if (!response.ok) throw new Error(`Failed to create the GitHub Release: ${response.status} ${await response.text()}`);
  logger.log(`Created the GitHub Release ${(await response.json()).html_url}`);
}

/** Returns `undefined` for an unpublished version, and an empty string when the registry records no commit. */
async function fetchPublishedCommit(url, getCommit) {
  const response = await fetch(url, { headers: { 'User-Agent': userAgent } });
  if (response.status === 404) return undefined;
  if (!response.ok) throw new Error(`GET ${url} failed: ${response.status} ${await response.text()}`);
  return getCommit(await response.json()) ?? '';
}
