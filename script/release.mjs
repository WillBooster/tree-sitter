// The release entry point. It runs semantic-release (through `wb release`) unless a failed run left a release pending
// at an older commit: the reusable workflow skips re-runs of a run whose commit is no longer the branch head, and
// semantic-release would compute the pending version again for the newer commit. The pending release is then
// completed first, in a run of this workflow dispatched on a temporary branch at its commit, because both registries
// attest the commit of the publishing run and the version tag must not exist before both hold the version; that run
// dispatches the workflow on the release branch again to release the newer commits.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

import {
  createGitHubClient,
  fetchPublishedCommits,
  findDraftRelease,
  listPendingReleases,
  publishRelease,
} from './releasePlugin.mjs';

const rootDir = path.resolve(import.meta.dirname, '..');
const releaseConfig = JSON.parse(fs.readFileSync(path.join(rootDir, '.releaserc.json'), 'utf8'));
const pluginConfig = releaseConfig.plugins.find(
  (plugin) => Array.isArray(plugin) && plugin[0] === './script/releasePlugin.mjs'
)[1];
const env = process.env;
const github = createGitHubClient(env);
const head = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: rootDir, encoding: 'utf8' }).trim();
// The registries trust this workflow file for publishing.
const dispatch = (ref) => github('POST', 'actions/workflows/release.yml/dispatches', { ref });
const pendingBranchPrefix = 'release-pending/';
// The dry-run options of `wb release` and of semantic-release (forwarded after `--`).
const dryRun = process.argv.slice(2).some((arg) => ['--dry-run', '--dry', '-d'].includes(arg));

if (!dryRun && env.GITHUB_REF_NAME.startsWith(pendingBranchPrefix)) {
  await completePendingRelease(env.GITHUB_REF_NAME.slice(pendingBranchPrefix.length));
  await dispatch(releaseConfig.branches[0]);
  // After the dispatch, since the reusable workflow skips re-runs on a deleted branch.
  await github('DELETE', `git/refs/heads/${env.GITHUB_REF_NAME}`);
} else if (!(await deferToPendingRelease()) || dryRun) {
  execFileSync('wb', ['release', ...process.argv.slice(2)], { cwd: rootDir, stdio: 'inherit' });
}

async function completePendingRelease(tag) {
  const draft = await findDraftRelease(github, tag);
  if (draft) {
    if (draft.target_commitish !== head) {
      throw new Error(`The draft release ${tag} targets ${draft.target_commitish}, not ${head}.`);
    }
    const version = tag.replace(/^v/, '');
    execFileSync(path.join(rootDir, 'script', 'build-release'), [version], { cwd: rootDir, stdio: 'inherit' });
    await publishRelease({ ...pluginConfig, cwd: rootDir, env, logger: console, draft, version });
  } else {
    // A previous attempt of this run published it; this attempt still hands over to the release branch.
    console.info(`The release ${tag} is not pending.`);
  }
}

/**
 * Returns whether a pending release of an older commit must be completed before releasing this commit. A dry run only
 * reports what a real run would do.
 */
async function deferToPendingRelease() {
  // Oldest first, since versions are released in order.
  const drafts = await listPendingReleases(github);
  for (const draft of drafts.toReversed()) {
    const commit = draft.target_commitish;
    const version = draft.tag_name.replace(/^v/, '');
    // semantic-release computes the same version again for the same commit and resumes the release itself.
    if (commit === head || !/^\d+\.\d+\.\d+/.test(version)) continue;

    const published = await fetchPublishedCommits({ ...pluginConfig, cwd: rootDir, version });
    if (published.every((target) => target.commit === undefined)) {
      // Nothing was released, so the version goes to the newer commits instead. A release that failed on a defect
      // (e.g., a packaging error) thus does not block the commit that fixes it.
      console.info(
        `${dryRun ? 'Would delete' : 'Deleting'} the draft release ${draft.tag_name} of ${commit}, which no registry holds`
      );
      if (!dryRun) await github('DELETE', `releases/${draft.id}`);
      continue;
    }

    const branch = `${pendingBranchPrefix}${draft.tag_name}`;
    if (dryRun) {
      console.info(`Would dispatch a run on ${branch} to complete the release before releasing this commit.`);
      return true;
    }
    await createBranch(branch, commit);
    await dispatch(branch);
    console.info(`Dispatched a run on ${branch} to complete the release; that run releases this commit next.`);
    return true;
  }
  return false;
}

async function createBranch(branch, commit) {
  try {
    await github('POST', 'git/refs', { ref: `refs/heads/${branch}`, sha: commit });
  } catch (error) {
    const existing = await github('GET', `git/ref/heads/${branch}`);
    if (existing.object.sha !== commit) throw error;
  }
}
