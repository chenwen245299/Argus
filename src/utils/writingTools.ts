import { invoke } from '@tauri-apps/api/core'
import { fragmentToMarkdown } from './copyAsMarkdown'
import { i18n } from '../i18n'

interface NoteEditor {
  read: (range: Range) => string
  value: () => string
  scope: () => string | null | undefined
  replace: (range: Range, markdown: string) => void
}
const editors = new WeakMap<HTMLElement, NoteEditor>()
const ANSWERS = '.answer-body.markdown-body, .assistant-bubble.markdown-body'

/** Keep Vditor's own Markdown/TeX conversion and native undo/input path. */
export function registerWritingToolsEditor(root: HTMLElement, editor: NoteEditor) {
  editors.set(root, editor)
  return () => { if (editors.get(root) === editor) editors.delete(root) }
}

export interface WritingToolsSelection {
  text: string
  replace?: (markdown: string) => boolean
}

export function captureWritingToolsSelection(target: Element): WritingToolsSelection | null {
  const selection = window.getSelection()
  if (!selection || selection.rangeCount !== 1 || selection.isCollapsed) return null
  const range = selection.getRangeAt(0).cloneRange()
  const element = range.commonAncestorContainer.nodeType === Node.ELEMENT_NODE
    ? range.commonAncestorContainer as Element : range.commonAncestorContainer.parentElement
  const root = element?.closest<HTMLElement>('.vditor-wrapper .vditor-reset, ' + ANSWERS)
  if (!root || !root.contains(target) || !root.contains(range.startContainer) || !root.contains(range.endContainer)) return null
  const editor = editors.get(root)
  if (editor && root.isContentEditable) {
    const text = editor.read(range)
    if (!text.trim()) return null
    const base = editor.value(), scope = editor.scope()
    return {
      text,
      replace(markdown) {
        // A native sheet can outlive a note, a library switch or a remote save.
        // Never relocate the result to a new selection or silently rebase it.
        if (!root.isConnected || !root.isContentEditable || editors.get(root) !== editor
          || editor.scope() !== scope || editor.value() !== base
          || !root.contains(range.startContainer) || !root.contains(range.endContainer)
          || editor.read(range) !== text) return false
        if (markdown === text) return true
        editor.replace(range, markdown)
        return true
      },
    }
  }
  if (!root.matches(ANSWERS)) return null
  const text = fragmentToMarkdown(range.cloneContents())
  // Answers are immutable here: a Writing Tools result is copied, never saved
  // back into model history, even if the native side were to request a replace.
  return text.trim() ? { text } : null
}

interface WritingToolsResult { action: 'cancel' | 'copy' | 'replace'; text?: string }
type NativeWritingTools = (text: string, canReplace: boolean, locale: string) => Promise<WritingToolsResult>
const nativeWritingTools: NativeWritingTools = (text, canReplace, locale) =>
  invoke('system_writing_tools', { text, canReplace, locale })

export async function runWritingTools(selection: WritingToolsSelection, native: NativeWritingTools = nativeWritingTools) {
  const result = await native(selection.text, !!selection.replace, i18n.global.locale.value)
  if (result.action !== 'replace') return true
  return typeof result.text === 'string' && !!selection.replace?.(result.text)
}

/** One policy for all windows; feature context menus retain precedence. */
export function installTextContextMenu() {
  const mac = /Mac/.test(navigator.platform) && '__TAURI_INTERNALS__' in window
  document.addEventListener('contextmenu', event => {
    if (event.defaultPrevented) return
    const target = event.target instanceof Element ? event.target : null
    if (!target) return
    if (mac) {
      const selection = captureWritingToolsSelection(target)
      if (selection) {
        event.preventDefault()
        void runWritingTools(selection).then(async replaced => {
          if (replaced) return
          const { message } = await import('@tauri-apps/plugin-dialog')
          await message(i18n.global.t('writingTools.changed'), { title: i18n.global.t('writingTools.title'), kind: 'warning' })
        }).catch(async error => {
          const { message } = await import('@tauri-apps/plugin-dialog')
          await message(i18n.global.t('writingTools.failed', { error: String(error) }), { title: i18n.global.t('writingTools.title'), kind: 'error' })
        })
        return
      }
    }
    // Keep native text editing menus, including nested contenteditable nodes.
    if (!target.closest('input,textarea') && !(target as HTMLElement).isContentEditable) event.preventDefault()
  })
}
