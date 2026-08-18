#!/usr/bin/env node
// Download the PDFium dynamic library for this platform into src-tauri/lib/, so
// the app can rasterise PDF pages in-process (view_paper_page) without the user
// installing poppler/pdftoppm. Runs at install/build time; the lib is then
// bundled into the app as a Tauri resource (tauri.conf.json → bundle.resources)
// and located at runtime by src-tauri/src/render.rs.
//
// Extraction is pure Node (gunzip + a tiny tar reader) so there is no dependency
// on a `tar` binary — that was the fragile part on the Windows CI runner.
//
// In CI (process.env.CI) a failure is fatal and loud: the app cannot be bundled
// without the lib. Locally it is non-fatal — page rendering just falls back to
// poppler at runtime. Idempotent: skips if the lib is already present.

import { copyFileSync, existsSync, mkdirSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { gunzipSync } from 'node:zlib'

const HERE = dirname(fileURLToPath(import.meta.url))
const LIB_DIR = join(HERE, '..', 'src-tauri', 'lib')
const IN_CI = !!process.env.CI

// bblanchon/pdfium-binaries asset → dest filename. macOS uses the universal
// binary so one file covers arm64 + x64.
const TARGETS = {
  'darwin':      ['pdfium-mac-univ.tgz', 'libpdfium.dylib'],
  'win32-x64':   ['pdfium-win-x64.tgz', 'pdfium.dll'],
  'win32-arm64': ['pdfium-win-arm64.tgz', 'pdfium.dll'],
  'linux-x64':   ['pdfium-linux-x64.tgz', 'libpdfium.so'],
  'linux-arm64': ['pdfium-linux-arm64.tgz', 'libpdfium.so'],
}

function done(msg) { console.log(`[fetch-pdfium] ${msg}`); process.exit(0) }
function fail(msg) {
  console[IN_CI ? 'error' : 'warn'](`[fetch-pdfium] ${msg}${IN_CI ? '' : ' — page rendering will fall back to poppler.'}`)
  process.exit(IN_CI ? 1 : 0)
}

const key = process.platform === 'darwin' ? 'darwin' : `${process.platform}-${process.arch}`
const target = TARGETS[key]
if (!target) fail(`no PDFium mapping for ${key}`)
const [asset, destName] = target
const dest = join(LIB_DIR, destName)

if (existsSync(dest) && statSync(dest).size > 100_000) done(`${destName} already present, skipping.`)

const url = `https://github.com/bblanchon/pdfium-binaries/releases/latest/download/${asset}`

async function download(u, attempts = 4) {
  let lastErr
  for (let i = 1; i <= attempts; i++) {
    try {
      const res = await fetch(u, { redirect: 'follow' })
      if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText}`)
      return Buffer.from(await res.arrayBuffer())
    } catch (e) {
      lastErr = e
      console.warn(`[fetch-pdfium] download attempt ${i}/${attempts} failed: ${e.message}`)
      if (i < attempts) await new Promise(r => setTimeout(r, 1500 * i))
    }
  }
  throw lastErr
}

// Extract one member (matched by basename) from an uncompressed tar buffer.
// The tar format is 512-byte headers followed by 512-padded file data.
function extractMemberByBasename(tar, base) {
  for (let off = 0; off + 512 <= tar.length; ) {
    const name = tar.toString('utf8', off, off + 100).replace(/\0.*$/, '')
    if (!name) break // zero block → end of archive
    const size = parseInt(tar.toString('utf8', off + 124, off + 136).replace(/[^0-7]/g, ''), 8) || 0
    const dataStart = off + 512
    if (size > 0 && name.split('/').pop() === base) {
      return tar.subarray(dataStart, dataStart + size)
    }
    off = dataStart + Math.ceil(size / 512) * 512
  }
  return null
}

try {
  console.log(`[fetch-pdfium] downloading ${url}`)
  const tar = gunzipSync(await download(url))
  const lib = extractMemberByBasename(tar, destName)
  if (!lib || lib.length < 100_000) throw new Error(`archive did not contain a usable ${destName}`)

  mkdirSync(LIB_DIR, { recursive: true })
  writeFileSync(dest, lib)
  console.log(`[fetch-pdfium] installed ${dest} (${(lib.length / 1e6).toFixed(1)} MB)`)
} catch (err) {
  fail(err.message)
}
