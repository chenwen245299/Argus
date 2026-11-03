import { confirm } from '@tauri-apps/plugin-dialog'
import { i18n } from '../i18n'

// The dialog plugin's window.confirm shim is asynchronous, and some versions
// still call its removed `confirm` command. Use the supported module API and
// await the actual choice before importing or discarding a duplicate.
let pendingDialog: Promise<unknown> = Promise.resolve()

export function confirmDuplicateImport(title: string, source?: string): Promise<boolean> {
  // PDF metadata and ebook imports can find duplicates at the same time. Native
  // sheets must not overlap: a click must resolve exactly the displayed file.
  const choice = pendingDialog.then(() => {
    const message = i18n.global.t('import.duplicateConfirm', { title })
    return confirm(source ? `${i18n.global.t('import.duplicateSource', { source })}\n\n${message}` : message, {
      title: i18n.global.t('import.duplicateTitle'),
      kind: 'warning',
      okLabel: i18n.global.t('import.keepDuplicate'),
      cancelLabel: i18n.global.t('import.skipDuplicate'),
    })
  })
  pendingDialog = choice.catch(() => {})
  return choice
}
