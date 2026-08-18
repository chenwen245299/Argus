<script setup lang="ts">
// A margin note in the app's tutorials and tips: hand-drawn icon on the left,
// handwriting on the right, wobbly ink border around it — so an explanation
// reads as something scribbled next to the UI rather than another grey box.
//
//   <DoodleNote icon="terminal" tone="tip" :title="t('…')">body text</DoodleNote>
//
// `icon` is a name from the doodle collection (see utils/doodleIcons.ts) and is
// optional — omit it for a note that is pure text. `tone` picks the ink colour:
// tip (accent, the default), warn (amber, for the gotchas), plain (grey, for
// asides). `tape` sticks a strip of washi tape over the top edge, which suits
// notes that open a section; skip it for inline ones.
import { computed } from 'vue'
import { Icon } from '@iconify/vue'

const props = withDefaults(defineProps<{
  icon?: string
  tone?: 'tip' | 'warn' | 'plain'
  title?: string
  tape?: boolean
  /** Degrees of tilt. The default keeps notes slightly off-axis; 0 sits square. */
  tilt?: number
}>(), { tone: 'tip', tape: false, tilt: -0.4 })

const style = computed(() => ({ '--note-tilt': `${props.tilt}deg` }))
</script>

<template>
  <div class="doodle-note" :class="tone" :style="style">
    <Icon v-if="tape" class="note-tape" icon="doodle:piece-washi-tape" width="52" height="16" />
    <Icon v-if="icon" class="note-icon" :icon="`doodle:${icon}`" width="30" height="30" />
    <div class="note-body">
      <span v-if="title" class="note-title">{{ title }}</span>
      <div class="note-text"><slot /></div>
    </div>
  </div>
</template>

<style scoped>
.doodle-note {
  position: relative;
  display: flex;
  align-items: flex-start;
  gap: 11px;
  padding: 12px 14px;
  /* Uneven corner radii are what sell the "drawn by hand" border. */
  border: 1.4px solid color-mix(in srgb, var(--note-ink) 38%, transparent);
  border-radius: 225px 14px 255px 15px / 15px 225px 16px 255px;
  background: color-mix(in srgb, var(--note-ink) 6%, var(--bg-primary));
  transform: rotate(var(--note-tilt));
  --note-ink: var(--accent);
}
.doodle-note.warn  { --note-ink: #d97706; }
.doodle-note.plain { --note-ink: var(--text-tertiary); }

/* A second, offset outline — the pen going round twice. */
.doodle-note::before {
  content: '';
  position: absolute;
  inset: 2px 1px 1px 2px;
  border: 1.1px solid color-mix(in srgb, var(--note-ink) 20%, transparent);
  border-radius: 15px 235px 15px 255px / 255px 15px 225px 15px;
  pointer-events: none;
}

.note-tape {
  position: absolute;
  top: -9px;
  left: 22px;
  color: color-mix(in srgb, var(--note-ink) 45%, var(--text-tertiary));
  transform: rotate(-7deg);
  opacity: 0.7;
}

.note-icon {
  flex-shrink: 0;
  margin-top: 1px;
  color: var(--note-ink);
  transform: rotate(calc(var(--note-tilt) * -2));
}

.note-body { min-width: 0; flex: 1; }
.note-title {
  display: block;
  margin-bottom: 3px;
  font-family: var(--font-hand);
  font-size: 14px;
  font-weight: 600;
  letter-spacing: 0.2px;
  color: color-mix(in srgb, var(--note-ink) 78%, var(--text-primary));
}
.note-text {
  font-family: var(--font-hand);
  font-size: 13.5px;
  line-height: 1.75;
  letter-spacing: 0.2px;
  color: var(--text-secondary);
}
/* Code inside a note stays monospace so paths and flags remain readable. */
.note-text :deep(code) {
  font-family: var(--font-mono);
  font-size: 11.5px;
  padding: 1px 5px;
  border-radius: var(--radius-sm);
  background: color-mix(in srgb, var(--note-ink) 10%, var(--bg-secondary));
  color: var(--text-primary);
}
.note-text :deep(strong) { color: var(--text-primary); font-weight: 600; }
</style>
