// pdf.js renders non-embedded standard fonts (Helvetica/Times/Symbol/…) and CID
// fonts only when it can fetch their data files. Without them, a PDF's shapes and
// embedded (e.g. LaTeX) text still draw, but text set in those fonts — figure
// labels, diagram captions — silently renders blank. Copy the data pdf.js ships
// into public/ so the viewer can point `standardFontDataUrl` / `cMapUrl` at it.
import { cpSync, mkdirSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

const __dirname = dirname(fileURLToPath(import.meta.url))
const pkg = join(__dirname, '../node_modules/pdfjs-dist')
const dest = join(__dirname, '../public/pdfjs')

for (const sub of ['standard_fonts', 'cmaps']) {
  mkdirSync(join(dest, sub), { recursive: true })
  cpSync(join(pkg, sub), join(dest, sub), { recursive: true, force: true })
}
console.log('pdfjs font/cmap assets copied → public/pdfjs/{standard_fonts,cmaps}')
