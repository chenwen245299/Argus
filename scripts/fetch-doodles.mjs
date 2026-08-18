// Fetches the hand-drawn icons used by the in-app tutorials/tips from
// koboyo.com and bakes them into an Iconify collection the app ships offline
// (src/assets/doodles/doodles.json, registered by src/utils/doodleIcons.ts).
//
//   node scripts/fetch-doodles.mjs
//
// Only the names listed below are downloaded — the whole koboyo set is 133k
// icons and its licence allows using icons as decoration inside a product, but
// not redistributing the collection itself or letting users browse/extract it.
// Add a name here when a new tip needs a drawing, then re-run.
//
// Source SVGs are single-path, `fill="currentColor"` and `viewBox="0 0 W H"`,
// so the conversion is just: strip the <svg> wrapper, keep the viewBox size.

import { writeFile, mkdir } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const BASE = 'https://koboyo.com/icons/svg'
const OUT = resolve(dirname(fileURLToPath(import.meta.url)), '../src/assets/doodles/doodles.json')

const ICONS = [
  // MCP / connecting an external agent
  'robot-helper', 'plug', 'plug-zap', 'handshake', 'terminal', 'config-file',
  'clipboard-copy', 'clipboard-tick', 'device-restarting', 'toggle-switch',
  // Safety / permissions
  'shield-lock', 'lock', 'key', 'folder-key',
  // The library itself
  'library', 'stack-books', 'paper-stack', 'highlighter', 'notebook',
  // Features that have their own tips
  'brain-circuit', 'vector-store', 'database', 'chat-panel', 'speech-bubble',
  'chatbot-suggesting', 'magnifier-loupe', 'map', 'target', 'rocket',
  'puzzle-piece', 'settings',
  // People — the empty states of the chat panels, where a figure reads better
  // than an object: someone reading a paper, someone talking through a diagram.
  'reading-document', 'explaining-whiteboard', 'having-realisation-while-studying',
  // …and one per empty sidebar panel, each showing the thing that panel fills up
  // with: looking a word up, writing a note, marking up an article, working
  // through a paper's sections, asking a question.
  'looking-up-word-dictionary', 'writing-notebook', 'person-highlighting-article',
  'person-highlighting-chapter', 'person-asking-question',
  // Note paper and margin scribbles
  'sticky-note', 'taped-note', 'torn-note', 'caveat-note', 'piece-washi-tape',
  'curved-arrow', 'pointing-finger', 'hand-drawn-underline', 'wavy-underline',
  'hand-drawn-scribble', 'sparkle-trio', 'bulb', 'lightbulb', 'check', 'warning',
]

/** Pulls the drawing out of a koboyo SVG file into an Iconify icon entry. */
function toIconifyIcon(svg, name) {
  const viewBox = svg.match(/viewBox="([^"]+)"/)?.[1]
  const body = svg.match(/<svg[^>]*>([\s\S]*)<\/svg>/)?.[1]
  if (!viewBox || !body) throw new Error(`${name}: unexpected SVG shape`)
  const [minX, minY, width, height] = viewBox.split(/\s+/).map(Number)
  if (minX !== 0 || minY !== 0) throw new Error(`${name}: unexpected viewBox origin`)
  return { body: body.trim(), width, height }
}

const icons = {}
for (const name of ICONS) {
  const res = await fetch(`${BASE}/${name}.svg`)
  if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`)
  icons[name] = toIconifyIcon(await res.text(), name)
  process.stdout.write(`· ${name}\n`)
}

await mkdir(dirname(OUT), { recursive: true })
await writeFile(OUT, JSON.stringify({ prefix: 'doodle', icons }, null, 0) + '\n')
console.log(`\n${Object.keys(icons).length} icons → ${OUT}`)
