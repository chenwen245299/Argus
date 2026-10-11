/** The backend's paper_ai_window_label encodes the UTF-8 slug in base64url.
 *  Read it from this window's immutable label, including after a reload. */
export function paperAiWindowSlug(label: string): string | null {
  const prefix = 'paper-ai-'
  if (!label.startsWith(prefix)) return null
  const encoded = label.slice(prefix.length)
  if (!/^[A-Za-z0-9_-]+$/.test(encoded)) return null
  try {
    const bytes = Uint8Array.from(atob(encoded.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0))
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes) || null
  } catch {
    return null
  }
}
