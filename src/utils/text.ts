export function titleInitialCaps(value: string | null | undefined): string {
  return (value ?? '').replace(/\b([a-z])/g, (letter) => letter.toUpperCase())
}
