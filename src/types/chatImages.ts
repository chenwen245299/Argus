/** A paper image saved in chats/<conversation id>/ and referenced by the answer. */
export interface AgentStepImage {
  slug: string
  page: number
  file?: string
  /** Live only. Dropped before persisting the conversation. */
  dataUrl?: string
  caption?: string
  width?: number
  height?: number
  dpi?: number
  /** Fractions of the rendered page, measured from the top-left corner. */
  crop?: { x: number; y: number; width: number; height: number } | null
}

export const SEND_PAPER_IMAGE_TOOL = 'send_paper_image'
