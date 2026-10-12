export interface SelectableTextContent {
  items: readonly {
    str?: string; dir?: string; fontName?: string; transform?: number[]
    width?: number; height?: number; hasEOL?: boolean
  }[]
  styles: Record<string, {
    fontFamily?: string; vertical?: boolean; ascent?: number; descent?: number
  } | undefined>
}

interface Viewport {
  width: number
  height: number
  scale: number
  transform: readonly number[]
}

interface LayerTask {
  render(): Promise<void>
  cancel(): void
}

export const TEXT_LAYER_TIMEOUT_MS = 1500
const MAX_TEXT_SPANS = 100_000 // Match PDF.js's text-layer limit.

/** A successful promise alone does not prove that the layer has selectable boxes. */
export function hasSelectableText(container: HTMLElement, content: SelectableTextContent): boolean {
  const items = content.items.filter(item => !!item.str)
  if (!items.length || items.length > MAX_TEXT_SPANS) return true
  const spans = [...container.querySelectorAll<HTMLElement>('span[role="presentation"]')]
  if (spans.length !== items.length || spans.some((span, i) => span.textContent !== items[i].str)) return false
  // A hidden/background tab cannot be measured. Its normal layer is kept until
  // the viewer shows it and renders at the current scale.
  if (!container.isConnected) return true
  const box = container.getBoundingClientRect()
  if (!box.width || !box.height) {
    return !container.parentElement?.getBoundingClientRect().width
  }
  let measured = false
  for (let i = 0; i < spans.length; i++) {
    const span = spans[i], item = items[i]
    // Decorative control glyphs and intentional zero-width runs do not make a
    // page unselectable. A real text run with missing boxes does, even when a
    // different font (e.g. the running header) still works.
    if (!/[\p{L}\p{N}\p{P}\p{S}]/u.test(item.str!) || !(item.width! > 0)) continue
    const rect = span.getBoundingClientRect()
    measured = true
    if (!Number.isFinite(rect.width) || !Number.isFinite(rect.height) || rect.width <= 0 || rect.height <= 0) return false
  }
  return measured || spans.some(span => span.getBoundingClientRect().width > 0)
}

/** Build transparent selection spans from the PDF's own text and coordinates.
 *  This does not OCR the bitmap, load fonts, or change the visible page pixels.
 *  DOM measurement avoids PDF.js's shared font-measuring canvas on the recovery path. */
export function buildTextLayerFallback(container: HTMLElement, content: SelectableTextContent, viewport: Viewport) {
  const doc = container.ownerDocument
  const fragment = doc.createDocumentFragment()
  const layouts: { span: HTMLSpanElement; width: number; height: number; angle: number }[] = []
  container.dataset.textLayerEngine = 'fallback'
  container.style.setProperty('--min-font-size', '1')
  container.style.setProperty('--total-scale-factor', String(viewport.scale))
  container.style.width = `calc(var(--total-scale-factor) * ${viewport.width / viewport.scale}px)`
  container.style.height = `calc(var(--total-scale-factor) * ${viewport.height / viewport.scale}px)`

  for (const item of content.items) {
    if (typeof item.str !== 'string') continue
    if (layouts.length >= MAX_TEXT_SPANS) break
    const m = item.transform, v = viewport.transform
    if (!m || m.length !== 6 || !m.every(Number.isFinite)) continue
    const tx = [
      v[0] * m[0] + v[2] * m[1], v[1] * m[0] + v[3] * m[1],
      v[0] * m[2] + v[2] * m[3], v[1] * m[2] + v[3] * m[3],
      v[0] * m[4] + v[2] * m[5] + v[4], v[1] * m[4] + v[3] * m[5] + v[5],
    ]
    const font = content.styles[item.fontName ?? '']
    const height = Math.hypot(tx[2], tx[3])
    if (!Number.isFinite(height) || height <= 0) continue
    const angle = Math.atan2(tx[1], tx[0]) + (font?.vertical ? Math.PI / 2 : 0)
    const ascent = font?.ascent && Number.isFinite(font.ascent) ? font.ascent
      : font?.descent && Number.isFinite(font.descent) ? 1 + font.descent : 0.8
    const left = tx[4] + height * ascent * Math.sin(angle)
    const top = tx[5] - height * ascent * Math.cos(angle)
    const width = Math.abs((font?.vertical ? item.height : item.width) ?? 0) * Math.hypot(v[0], v[1])
    if (item.str) {
      const span = doc.createElement('span')
      span.setAttribute('role', 'presentation')
      span.textContent = item.str
      span.dir = item.dir ?? 'ltr'
      span.style.left = `${100 * left / viewport.width}%`
      span.style.top = `${100 * top / viewport.height}%`
      span.style.fontFamily = font?.fontFamily || 'sans-serif'
      span.style.fontSize = `calc(var(--total-scale-factor) * ${height / viewport.scale}px)`
      span.style.transform = 'none'
      fragment.append(span)
      layouts.push({ span, width, height, angle })
    }
    if (item.hasEOL) {
      const br = doc.createElement('br')
      br.setAttribute('role', 'presentation')
      fragment.append(br)
    }
  }
  container.replaceChildren(fragment)
  // Read all boxes before writing transforms; only one layout pass per page.
  const boxes = layouts.map(({ span }) => span.getBoundingClientRect())
  layouts.forEach(({ span, width, height, angle }, i) => {
    const box = boxes[i]
    const sx = box.width > 0 && width > 0 ? width / box.width : 1
    const sy = box.height > 0 ? height / box.height : 1
    span.style.transform = `rotate(${angle * 180 / Math.PI}deg) scale(${sx}, ${sy})`
  })
}

/** Keep the normal engine when it works; recover failures, empty geometry and
 *  rejected/hung font layout. A retired task retains only its detached container. */
export async function renderSelectableTextLayer(options: {
  container: HTMLDivElement
  content: SelectableTextContent
  viewport: Viewport
  create: (container: HTMLDivElement) => LayerTask
  cancelled: () => boolean
}): Promise<HTMLDivElement> {
  const { container, content, viewport, create, cancelled } = options
  let task: LayerTask | null = null
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    task = create(container)
    await Promise.race([
      task.render(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('PDF text layer layout timed out')), TEXT_LAYER_TIMEOUT_MS)
      }),
    ])
    if (cancelled()) { task.cancel(); return container }
    if (hasSelectableText(container, content)) return container
  } catch (error) {
    if (!cancelled()) console.warn('PDF text layer recovery:', error)
  } finally {
    if (timer !== undefined) clearTimeout(timer)
  }
  try { task?.cancel() } catch { /* an already completed task */ }
  if (cancelled()) return container
  const fallback = container.cloneNode(false) as HTMLDivElement
  container.replaceWith(fallback)
  buildTextLayerFallback(fallback, content, viewport)
  return fallback
}
