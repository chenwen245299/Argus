import { test } from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { readReleaseTag } from './release-notes.mjs'

function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), 'release-notes-test-'))
  t.after(() => rmSync(cwd, { recursive: true, force: true }))
  const git = (...args) => execFileSync('git', [
    '-c', 'user.name=Release fixture', '-c', 'user.email=fixture@example.invalid',
    '-c', 'commit.gpgSign=false', '-c', 'tag.gpgSign=false',
    '-c', 'core.hooksPath=disabled-hooks', ...args,
  ], { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trimEnd()
  git('init', '--quiet')
  git('commit', '--quiet', '--allow-empty', '-m', 'A short commit summary')
  const sha = git('rev-parse', 'HEAD')
  function tag(name, notes) {
    const file = join(cwd, 'notes.md')
    writeFileSync(file, notes)
    git('tag', '-a', name, '--cleanup=verbatim', '-F', file)
  }
  return { cwd, git, sha, tag }
}

test('annotated tag preserves complete Markdown independently of the commit message and current HEAD', t => {
  const f = fixture(t)
  const notes = '# Release Notes\n\n## ✨ New Features\n\n- **引用**：支持 `/ref`。\n\nEOF\n`$(literal text)`\n'
  f.tag('v1.2.0', notes)
  f.git('commit', '--quiet', '--allow-empty', '-m', 'A later unrelated commit')
  assert.deepEqual(readReleaseTag('v1.2.0', f.cwd), {
    tag: 'v1.2.0', version: '1.2.0', sha: f.sha, notes: notes.trim(), prerelease: false,
  })
})

test('lightweight tag never falls back to the commit message', t => {
  const f = fixture(t)
  f.git('tag', 'v1.2.0')
  assert.throws(() => readReleaseTag('v1.2.0', f.cwd), /lightweight.*git tag -a/)
})

test('missing tag and empty annotation fail before a release can be created', t => {
  const f = fixture(t)
  assert.throws(() => readReleaseTag('v1.2.0', f.cwd), /does not exist/)
  f.tag('v1.2.0', ' \n\n')
  assert.throws(() => readReleaseTag('v1.2.0', f.cwd), /no release notes/)
})

test('prerelease status comes from the version, including optional build metadata', t => {
  const f = fixture(t)
  f.tag('v1.2.0-beta.1+build.4', 'Preview notes')
  assert.equal(readReleaseTag('v1.2.0-beta.1+build.4', f.cwd).prerelease, true)
  f.tag('v1.2.0+build.4', 'Stable notes')
  assert.equal(readReleaseTag('v1.2.0+build.4', f.cwd).prerelease, false)
})

test('invalid versions and input resembling commands are rejected', () => {
  for (const tag of ['', '1.2.0', 'v1.2', 'v01.2.0', 'v1.2.0-01', 'v1.2.0-', 'v1.2.0..', 'v1.2.0/extra', 'v1.2.0\nother', 'v1.2.0\n', 'v1.2.0; touch file']) {
    assert.throws(() => readReleaseTag(tag), /Use a version tag/)
  }
})

test('tag signatures stay out of the displayed release notes', t => {
  const f = fixture(t)
  for (const [i, kind] of ['PGP SIGNATURE', 'SSH SIGNATURE', 'SIGNED MESSAGE'].entries()) {
    f.tag(`v1.2.${i}`, `# Release Notes\n\n- 更新内容\n\n-----BEGIN ${kind}-----\nfixture\n-----END ${kind}-----\n`)
    assert.equal(readReleaseTag(`v1.2.${i}`, f.cwd).notes, '# Release Notes\n\n- 更新内容')
  }
})
