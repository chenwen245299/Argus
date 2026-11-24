// Release notes belong to the annotated tag, independently of its commit message.
import { execFileSync } from 'node:child_process'

export function readReleaseTag(tag, cwd = process.cwd()) {
  const match = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(tag || '')
  if (!match || match[0] !== tag || match[4]?.split('.').some(part => /^0\d+$/.test(part))) {
    throw new Error('Use a version tag such as v1.2.0 or v1.2.0-beta.1.')
  }

  const git = (...args) => execFileSync('git', args, {
    cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
  }).trimEnd()
  let object
  try {
    object = git('rev-parse', '--verify', `refs/tags/${tag}`)
  } catch {
    throw new Error(`Tag ${tag} does not exist. Push the annotated tag before running the release workflow.`)
  }
  if (git('cat-file', '-t', object) !== 'tag') {
    throw new Error(`Tag ${tag} is lightweight. Create an annotated tag with git tag -a <version> --cleanup=verbatim -F <release-notes-file>.`)
  }

  // Read the resolved object so the notes and source stay tied to the same tag.
  const raw = git('cat-file', 'tag', object)
  const separator = raw.indexOf('\n\n')
  const notes = (separator === -1 ? '' : raw.slice(separator + 2))
    .replace(/^-----BEGIN (?:PGP SIGNATURE|SSH SIGNATURE|SIGNED MESSAGE)-----\r?\n[\s\S]*$/m, '')
    .trim()
  if (!notes) throw new Error(`Tag ${tag} has no release notes.`)

  return {
    tag,
    version: tag.slice(1),
    sha: git('rev-parse', '--verify', `${object}^{commit}`),
    notes,
    prerelease: Boolean(match[4]),
  }
}
