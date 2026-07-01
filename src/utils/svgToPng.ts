export function svgStringToPngBlob(svgString: string): Promise<Blob> {
  return new Promise((resolve, reject) => {
    const parser = new DOMParser()
    const doc = parser.parseFromString(svgString, 'image/svg+xml')
    const svgEl = doc.documentElement

    let width = parseFloat(svgEl.getAttribute('width') || '0')
    let height = parseFloat(svgEl.getAttribute('height') || '0')

    if (!width || !height) {
      const vb = svgEl.getAttribute('viewBox')
      if (vb) {
        const parts = vb.split(/[\s,]+/).map(Number)
        width = parts[2] || 0
        height = parts[3] || 0
      }
    }

    width = Math.max(1, width || 800)
    height = Math.max(1, height || 600)

    let fixed = svgString
    if (!fixed.includes('xmlns=')) {
      fixed = fixed.replace('<svg', '<svg xmlns="http://www.w3.org/2000/svg"')
    }
    if (!fixed.match(/<svg[^>]*\swidth=/)) {
      fixed = fixed.replace('<svg', `<svg width="${width}" height="${height}"`)
    }

    const blob = new Blob([fixed], { type: 'image/svg+xml;charset=utf-8' })
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
      ctx.drawImage(img, 0, 0)
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
