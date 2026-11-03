// pdf.js renders non-embedded standard fonts (Helvetica/Times/Symbol/…) and CID
// fonts only when it can fetch their data files. Without them, a PDF's shapes and
// embedded (e.g. LaTeX) text still draw, but text set in those fonts — figure
// labels, diagram captions — silently renders blank. Copy the data pdf.js ships
// into public/ so the viewer can point `standardFontDataUrl` / `cMapUrl` at it.
// Scanned books also need the shipped JPEG 2000 / JBIG2 decoders (WASM and JS
// fallback) and ICC color profiles. Missing codecs silently drop page images
// while page.render() still succeeds, leaving a white page with selectable OCR.
import { cpSync, mkdirSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const pkg = join(__dirname, '../node_modules/pdfjs-dist')
const dest = join(__dirname, '../public/pdfjs')

for (const sub of ['standard_fonts', 'cmaps', 'wasm', 'iccs']) {
  mkdirSync(join(dest, sub), { recursive: true })
  cpSync(join(pkg, sub), join(dest, sub), { recursive: true, force: true })
}
console.log('pdfjs fonts, image decoders and ICC profiles copied → public/pdfjs')
