import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

const workflow = readFileSync(new URL('../.github/workflows/release.yml', import.meta.url), 'utf8')
const createStep = workflow.split('      - name: Create release\n')[1]?.split('\n  build-tauri:')[0]
assert.ok(createStep, 'The release creation step must exist')
const script = createStep.split('          script: |\n')[1]
  .split('\n').map(line => line.startsWith('            ') ? line.slice(12) : line).join('\n')
const execute = new (Object.getPrototypeOf(async function () {}).constructor)('github', 'context', 'process', script)

function fixture(releases) {
  const calls = []
  const repo = { owner: 'fixture', repo: 'argus' }
  const env = {
    TAG_NAME: 'v1.2.0', RELEASE_BODY: '# Release Notes\n\n- 完整更新日志。',
    PRERELEASE: 'false', COMMIT_SHA: 'a'.repeat(40),
  }
  const write = method => async params => {
    // GitHub checks workflow-write access for an older target whose workflows
    // differ from the default branch, even when the tag already exists.
    if (params.target_commitish === env.COMMIT_SHA) {
      throw Object.assign(new Error('Resource not accessible by integration'), { status: 403 })
    }
    calls.push({ method, params })
    return { data: { id: params.release_id || 123 } }
  }
  const github = {
    paginate: async () => releases,
    rest: { repos: { listReleases() {}, createRelease: write('create'), updateRelease: write('update') } },
  }
  return { calls, env, run: () => execute(github, { repo }, { env }) }
}

test('an existing historical tag can create a draft with contents-write access', async () => {
  const f = fixture([])
  assert.equal(await f.run(), 123)
  assert.deepEqual(f.calls, [{ method: 'create', params: {
    owner: 'fixture', repo: 'argus', tag_name: 'v1.2.0', name: 'v1.2.0',
    body: f.env.RELEASE_BODY, draft: true, prerelease: false,
  } }])
})

test('retry reuses the matching draft and retains prerelease notes without requesting workflow-write access', async () => {
  const f = fixture([{ id: 5, tag_name: 'v1.1.0', draft: true }, { id: 9, tag_name: 'v1.2.0', draft: true }])
  f.env.PRERELEASE = 'true'
  assert.equal(await f.run(), 9)
  assert.deepEqual(f.calls, [{ method: 'update', params: {
    owner: 'fixture', repo: 'argus', tag_name: 'v1.2.0', name: 'v1.2.0',
    body: f.env.RELEASE_BODY, draft: true, prerelease: true, release_id: 9,
  } }])
})

test('an already published version is never rewritten', async () => {
  const f = fixture([{ id: 9, tag_name: 'v1.2.0', draft: false }])
  await assert.rejects(f.run(), /already published/)
  assert.deepEqual(f.calls, [])
})
