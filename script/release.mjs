// The release entry point. It runs semantic-release (through `wb release`) unless a failed run left a release pending
// at an older commit: the reusable workflow skips re-runs of a run whose commit is no longer the branch head, and
// semantic-release would compute the pending version again for the newer commit. The pending release is then
// completed first, in a run of this workflow dispatched on its tag, because both registries attest the commit of
// the publishing run; that run dispatches the workflow on the branch again to release the newer commits.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

import { createGitHubClient, fetchPublishedCommits, listDraftReleases, publishRelease } from './releasePlugin.mjs';

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

if (env.GITHUB_REF_TYPE === 'tag') {
  await completePendingRelease(env.GITHUB_REF_NAME);
} else if (!(await deferToPendingRelease())) {
  execFileSync('wb', ['release', ...process.argv.slice(2)], { cwd: rootDir, stdio: 'inherit' });
}

async function completePendingRelease(tag) {
  const draft = (await listDraftReleases(github)).find((release) => release.tag_name === tag);
  if (!draft) {
    console.info(`The release ${tag} is not pending.`);
    return;
  }
  if (draft.target_commitish !== head) {
    throw new Error(`The draft release ${tag} targets ${draft.target_commitish}, not ${head}.`);
  }
  const version = tag.replace(/^v/, '');
  execFileSync(path.join(rootDir, 'script', 'build-release'), [version], { cwd: rootDir, stdio: 'inherit' });
  await publishRelease({ ...pluginConfig, cwd: rootDir, env, logger: console, draft, version });
  await dispatch(releaseConfig.branches[0]);
}

/** Returns whether a pending release of an older commit must be completed before releasing this commit. */
async function deferToPendingRelease() {
  // Oldest first, since versions are released in order.
  for (const draft of (await listDraftReleases(github)).toReversed()) {
    const commit = draft.target_commitish;
    const version = draft.tag_name.replace(/^v/, '');
    // semantic-release computes the same version again for the same commit and resumes the release itself.
    if (commit === head || !/^\d+\.\d+\.\d+/.test(version)) continue;

    const published = await fetchPublishedCommits({ ...pluginConfig, cwd: rootDir, version });
    if (published.every((target) => target.commit === undefined)) {
      // Nothing was released, so the version goes to the newer commits instead. A release that failed on a defect
      // (e.g., a packaging error) thus does not block the commit that fixes it.
      console.info(`Deleting the draft release ${draft.tag_name} of ${commit}, which no registry holds`);
      await github('DELETE', `releases/${draft.id}`);
      continue;
    }

    await createTag(draft.tag_name, commit);
    await dispatch(draft.tag_name);
    console.info(`Dispatched a run on ${draft.tag_name} to complete its release; that run releases this commit next.`);
    return true;
  }
  return false;
}

async function createTag(tag, commit) {
  try {
    await github('POST', 'git/refs', { ref: `refs/tags/${tag}`, sha: commit });
  } catch (error) {
    const existing = await github('GET', `git/ref/tags/${tag}`);
    if (existing.object.sha !== commit) throw error;
  }
}
