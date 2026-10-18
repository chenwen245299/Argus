export interface GuidedReadingPreset {
  id: string
  name: string
  prompt: string
}

export interface GuidedReadingSettings {
  presets: GuidedReadingPreset[]
  defaultId: string
}

export const BUILTIN_GUIDED_READING_ID = 'builtin'
