#!/usr/bin/env node
// Download the PDFium dynamic library for this platform into src-tauri/lib/, so
// the app can rasterise PDF pages in-process (view_paper_page) without the user
// installing poppler/pdftoppm. Runs at install/build time; the lib is then
// bundled into the app as a Tauri resource (see tauri.conf.json → bundle.
// resources) and located at runtime by src-tauri/src/render.rs.
//
// Idempotent: skips if the lib is already present. Never hard-fails the install
// — a missing lib just means page rendering falls back to poppler at runtime.

import { execFileSync } from 'node:child_process'
import {
  copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const LIB_DIR = join(HERE, '..', 'src-tauri', 'lib')

// bblanchon/pdfium-binaries asset → [tgz asset, path inside the archive, dest
// filename]. macOS uses the universal binary so one file covers arm64 + x64.
const TARGETS = {
  'darwin':      ['pdfium-mac-univ.tgz', 'lib/libpdfium.dylib', 'libpdfium.dylib'],
  'win32-x64':   ['pdfium-win-x64.tgz', 'bin/pdfium.dll', 'pdfium.dll'],
  'win32-arm64': ['pdfium-win-arm64.tgz', 'bin/pdfium.dll', 'pdfium.dll'],
  'linux-x64':   ['pdfium-linux-x64.tgz', 'lib/libpdfium.so', 'libpdfium.so'],
  'linux-arm64': ['pdfium-linux-arm64.tgz', 'lib/libpdfium.so', 'libpdfium.so'],
}

const key = process.platform === 'darwin' ? 'darwin' : `${process.platform}-${process.arch}`
const target = TARGETS[key]
if (!target) {
  console.warn(`[fetch-pdfium] no PDFium mapping for ${key}; page rendering will fall back to poppler.`)
  process.exit(0)
}
const [asset, innerPath, destName] = target
const dest = join(LIB_DIR, destName)

if (existsSync(dest) && statSync(dest).size > 100_000) {
  console.log(`[fetch-pdfium] ${destName} already present, skipping.`)
  process.exit(0)
}

const url = `https://github.com/bblanchon/pdfium-binaries/releases/latest/download/${asset}`
const tmp = mkdtempSync(join(tmpdir(), 'argus-pdfium-'))

try {
  console.log(`[fetch-pdfium] downloading ${url}`)
  const res = await fetch(url, { redirect: 'follow' })
  if (!res.ok) throw new Error(`download failed: ${res.status} ${res.statusText}`)

  const tgz = join(tmp, asset)
  writeFileSync(tgz, Buffer.from(await res.arrayBuffer()))

  // `tar` ships on macOS, Linux, and modern Windows (bsdtar). Extract just the lib.
  execFileSync('tar', ['-xzf', tgz, '-C', tmp, innerPath], { stdio: 'inherit' })

  mkdirSync(LIB_DIR, { recursive: true })
  copyFileSync(join(tmp, innerPath), dest) // copy, not rename: tmp is a different mount
  console.log(`[fetch-pdfium] installed ${dest} (${(statSync(dest).size / 1e6).toFixed(1)} MB)`)
} catch (err) {
  console.warn(`[fetch-pdfium] ${err.message}; page rendering will fall back to poppler.`)
} finally {
  rmSync(tmp, { recursive: true, force: true })
}
