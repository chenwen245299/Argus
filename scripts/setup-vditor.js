import { cpSync, mkdirSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'
const __dirname = dirname(fileURLToPath(import.meta.url))
const src = join(__dirname, '../node_modules/vditor/dist')
const dest = join(__dirname, '../public/vditor/dist')
mkdirSync(dest, { recursive: true })
cpSync(src, dest, { recursive: true, force: true })
console.log('vditor assets copied → public/vditor/dist')
