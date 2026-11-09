import { test } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'

test('native Writing Tools handoff works without an Apple Intelligence model', { skip: process.platform !== 'darwin' }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'argus-writing-tools-'))
  try {
    const binary = join(directory, 'native-test')
    const compiled = spawnSync('xcrun', ['clang', '-fobjc-arc', '-Wall', '-Werror', '-mmacosx-version-min=11.0',
      '-framework', 'AppKit', new URL('./test-writing-tools-native.m', import.meta.url).pathname, '-o', binary], { encoding: 'utf8' })
    assert.equal(compiled.status, 0, compiled.stderr)
    const result = spawnSync(binary, process.env.ARGUS_TEST_OUTPUT_DIR ? [join(process.env.ARGUS_TEST_OUTPUT_DIR, 'writing-tools-native.png')] : [], { encoding: 'utf8', timeout: 20000 })
    assert.equal(result.status, 0, result.stderr || result.stdout)
    console.log(result.stdout.trim())
  } finally { await rm(directory, { recursive: true, force: true }) }
})
