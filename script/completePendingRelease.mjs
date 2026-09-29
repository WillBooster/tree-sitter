// Completes the releases that failed runs left as drafts before a newer commit reached the branch. Runs before
// semantic-release, which would otherwise compute the pending version again for the newer commit and find it published
// from another commit. A draft whose registries hold nothing is deleted instead, so that a release failing on a
// defect (e.g., a packaging error) does not block the commit that fixes it.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

import { createGitHubClient, fetchPublishedCommits, listDraftReleases, publishRelease } from './releasePlugin.mjs';

const rootDir = path.resolve(import.meta.dirname, '..');
const pluginConfig = JSON.parse(fs.readFileSync(path.join(rootDir, '.releaserc.json'), 'utf8')).plugins.find(
  (plugin) => Array.isArray(plugin) && plugin[0] === './script/releasePlugin.mjs'
)[1];
const git = (...args) => execFileSync('git', args, { cwd: rootDir, encoding: 'utf8' }).trim();
const github = createGitHubClient(process.env);
const head = git('rev-parse', 'HEAD');

// Oldest first, so that the latest completed release becomes the latest GitHub Release.
for (const draft of (await listDraftReleases(github)).toReversed()) {
  const commit = draft.target_commitish;
  const version = draft.tag_name.replace(/^v/, '');
  // semantic-release computes the same version again for the same commit and resumes the release itself.
  if (commit === head || !/^\d+\.\d+\.\d+/.test(version)) continue;

  const published = await fetchPublishedCommits({ ...pluginConfig, cwd: rootDir, version });
  if (published.every((target) => target.commit === undefined)) {
    console.info(`Deleting the draft release ${draft.tag_name} of ${commit}, which no registry holds`);
    await github('DELETE', `releases/${draft.id}`);
    continue;
  }

  console.info(`Completing the release ${draft.tag_name} of ${commit}`);
  const worktree = path.join(rootDir, '.tmp', `release-${version}`);
  git('fetch', '--depth=1', 'origin', commit);
  git('worktree', 'add', '--force', '--detach', worktree, commit);
  execFileSync(path.join(worktree, 'script', 'build-release'), [version], { cwd: worktree, stdio: 'inherit' });
  await publishRelease({ ...pluginConfig, cwd: worktree, env: process.env, logger: console, draft, version });
  git('worktree', 'remove', '--force', worktree);
}
