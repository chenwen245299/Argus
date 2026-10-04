// Point git at the committed hooks in .githooks/ so a fresh clone is protected by
// the gitleaks secret scan (pre-commit and pre-push) after its first `npm install`.
// `core.hooksPath` is per-clone git config, which is why this has to be a script
// rather than something the repository can ship.
//
// Never fails the install: no .git (a source tarball), no git binary, a CI runner
// (CI scans in its own workflow), or a hooksPath the developer set themselves are
// all quiet no-ops.
import { execFileSync } from 'child_process'
import { chmodSync, existsSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const HOOKS_DIR = '.githooks'

function git(...args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim()
}

function main() {
  if (process.env.CI) return
  if (!existsSync(join(root, '.git'))) return

  // A zip download or a checkout on a filesystem without modes loses the
  // executable bit, and git silently ignores a hook that is not executable.
  for (const hook of ['pre-commit', 'pre-push']) {
    try { chmodSync(join(root, HOOKS_DIR, hook), 0o755) } catch { /* not there: nothing to arm */ }
  }

  let current = ''
  try { current = git('config', '--local', '--get', 'core.hooksPath') } catch { /* unset */ }
  if (current === HOOKS_DIR) return
  if (current) {
    console.log(`git hooks: core.hooksPath is already "${current}", leaving it alone — secret scanning is NOT enabled (see ${HOOKS_DIR}/)`)
    return
  }
  git('config', '--local', 'core.hooksPath', HOOKS_DIR)
  console.log(`git hooks: enabled ${HOOKS_DIR}/ (gitleaks secret scan on commit and push)`)

  try { execFileSync('gitleaks', ['version'], { stdio: 'ignore' }) } catch {
    console.log('git hooks: gitleaks is not installed — commits will be refused until it is (brew install gitleaks)')
  }
}

try { main() } catch { /* a hook that could not be installed must not break npm install */ }
