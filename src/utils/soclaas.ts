// NUS Computing's official favicon, kept locally for offline provider pickers.
// Source: https://www.comp.nus.edu.sg/wp-content/uploads/2023/09/cropped-favicon-192x192.png
import nusIcon from '../assets/providers/nus.png'

export const SOCLAAS_BASE_URL = 'https://soclaas-api.comp.nus.edu.sg/v1'

export function isSoclaas(name = '', baseUrl = '', kind = ''): boolean {
  const text = `${name} ${baseUrl}`.toLowerCase()
  return kind === 'soclaas' || text.includes('soclaas') || text.includes('soclass')
}

export function soclaasLogo(name = '', baseUrl = '', kind = ''): string {
  return isSoclaas(name, baseUrl, kind) ? nusIcon : ''
}
