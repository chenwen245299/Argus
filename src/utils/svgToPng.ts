/** Resolve percentage/auto sizing using the viewBox, never parse 100% as 100px. */
export function normalizeSvgDimensions(svgEl: Element): { width: number; height: number } {
  const length = (name: string) => {
    const value = svgEl.getAttribute(name)?.trim() ?? ''
    const match = value.match(/^(\d+(?:\.\d*)?|\.\d+)(px|in|cm|mm|pt|pc)?$/i)
    if (!match) return 0
    const units: Record<string, number> = { px: 1, in: 96, cm: 96 / 2.54, mm: 96 / 25.4, pt: 96 / 72, pc: 16 }
    const number = Number(match[1]) * units[(match[2] ?? 'px').toLowerCase()]
    return Number.isFinite(number) && number > 0 ? number : 0
  }
  const vb = (svgEl.getAttribute('viewBox') ?? '').trim().split(/[\s,]+/).map(Number)
  const hasViewBox = vb.length === 4 && vb.every(Number.isFinite) && vb[2] > 0 && vb[3] > 0
  let width = length('width'), height = length('height')
  if (!width && !height) {
    width = hasViewBox ? vb[2] : 800
    height = hasViewBox ? vb[3] : 600
  } else if (!width) width = hasViewBox ? height * vb[2] / vb[3] : 800
  else if (!height) height = hasViewBox ? width * vb[3] / vb[2] : 600
  // Bound rasterization memory, retaining the original coordinate system.
  const scale = Math.min(1, 8192 / Math.max(width, height), Math.sqrt(16_000_000 / (width * height)))
  if (!hasViewBox) svgEl.setAttribute('viewBox', `0 0 ${width} ${height}`)
  width = Math.max(1, Math.round(width * scale))
  height = Math.max(1, Math.round(height * scale))
  svgEl.setAttribute('xmlns', 'http://www.w3.org/2000/svg')
  svgEl.setAttribute('width', String(width))
  svgEl.setAttribute('height', String(height))
  // A root style such as height:100vh must not override the resolved viewport.
  const style = document.createElement('div').style
  style.cssText = svgEl.getAttribute('style') ?? ''
  for (const property of ['width', 'height', 'min-width', 'min-height', 'max-width', 'max-height']) style.removeProperty(property)
  if (style.cssText) svgEl.setAttribute('style', style.cssText)
  else svgEl.removeAttribute('style')
  return { width, height }
}

export function svgStringToPngBlob(svgString: string): Promise<Blob> {
  return new Promise((resolve, reject) => {
    const parser = new DOMParser()
    const doc = parser.parseFromString(svgString, 'image/svg+xml')
    const svgEl = doc.documentElement

    if (svgEl.localName !== 'svg' || doc.querySelector('parsererror')) {
      reject(new Error('Invalid SVG'))
      return
    }
    const { width, height } = normalizeSvgDimensions(svgEl)
    const blob = new Blob([new XMLSerializer().serializeToString(svgEl)], { type: 'image/svg+xml;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const img = new Image()

    img.onload = () => {
      const canvas = document.createElement('canvas')
      canvas.width = width
      canvas.height = height
      const ctx = canvas.getContext('2d')
      if (!ctx) {
        URL.revokeObjectURL(url)
        reject(new Error('Failed to get 2D canvas context'))
        return
      }
      ctx.drawImage(img, 0, 0, width, height)
      URL.revokeObjectURL(url)
      canvas.toBlob(
        b => (b ? resolve(b) : reject(new Error('canvas.toBlob returned null'))),
        'image/png',
      )
    }

    img.onerror = () => {
      URL.revokeObjectURL(url)
      reject(new Error('Failed to load SVG into Image'))
    }

    img.src = url
  })
}
