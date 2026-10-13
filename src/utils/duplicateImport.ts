import { confirm } from '@tauri-apps/plugin-dialog'
import { i18n } from '../i18n'

// The dialog plugin's window.confirm shim is asynchronous, and some versions
// still call its removed `confirm` command. Use the supported module API and
// await the actual choice before importing or discarding a duplicate.
export async function confirmDuplicateImport(title: string): Promise<boolean> {
  return await confirm(i18n.global.t('import.duplicateConfirm', { title }), {
    title: i18n.global.t('import.duplicateTitle'),
    kind: 'warning',
    okLabel: i18n.global.t('import.keepDuplicate'),
    cancelLabel: i18n.global.t('import.skipDuplicate'),
  })
}
