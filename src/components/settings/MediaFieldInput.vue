<script setup lang="ts">
/**
 * One control of a form that is DATA, not code.
 *
 * A media adapter describes each knob of a model as a `MediaField` (key, label,
 * kind, options, default, range, note); this renders it. So a provider added later
 * brings its own voices, speeds and toggles with no change here.
 *
 * The value is committed, not streamed: a select or a switch emits at once, but a
 * text or number box emits when it loses focus or Enter is pressed — each emit is
 * a settings write, and a write per keystroke is not what anyone wants. An empty
 * box emits `undefined`, which the owner reads as "the provider's own default".
 *
 * The Media Studio renders the same five kinds inline with its own styling; this
 * is the settings-panel twin, kept separate so neither page's look changes.
 */
import { ref, watch, useId, nextTick } from 'vue'
import type { MediaField } from '../../types'

const props = defineProps<{
  field: MediaField
  /** The current value, or undefined when unset. */
  modelValue: unknown
}>()
const emit = defineEmits<{ 'update:modelValue': [value: unknown] }>()

const id = useId()

function asText(v: unknown): string {
  return v === undefined || v === null ? '' : String(v)
}

/** What a text / number box shows while it is being typed in. */
const draft = ref(asText(props.modelValue))
watch(() => props.modelValue, (v) => { draft.value = asText(v) })

// Enter and the blur that follows it both commit; the second one finds nothing
// new to say, and must not write the settings file again.
function commitText() {
  const v = draft.value.trim()
  if (v === asText(props.modelValue)) return
  emit('update:modelValue', v === '' ? undefined : v)
}

function commitNumber() {
  const raw = draft.value.trim()
  if (raw === '') {
    // Cleared = "use the provider's own default". The owner hands that default back
    // as the value, which may be the very number already shown, so the watcher
    // above would not fire: put it back in the box explicitly.
    emit('update:modelValue', undefined)
    void nextTick(() => { draft.value = asText(props.modelValue) })
    return
  }
  let n = Number(raw)
  if (!Number.isFinite(n)) {
    // Not a number: put back what was there rather than saving nonsense.
    draft.value = asText(props.modelValue)
    return
  }
  const { min, max } = props.field
  if (min !== undefined && n < min) n = min
  if (max !== undefined && n > max) n = max
  draft.value = String(n)
  if (props.modelValue !== n) emit('update:modelValue', n)
}
</script>

<template>
  <div class="mfi" :class="{ wide: field.kind === 'long_text' }">
    <label class="mfi-label" :for="id">{{ field.label }}</label>

    <select
      v-if="field.kind === 'select'"
      :id="id"
      class="mfi-input"
      :value="asText(modelValue)"
      @change="emit('update:modelValue', ($event.target as HTMLSelectElement).value)"
    >
      <option v-for="o in field.options" :key="o.value" :value="o.value">{{ o.label }}</option>
    </select>

    <textarea
      v-else-if="field.kind === 'long_text'"
      :id="id"
      v-model="draft"
      class="mfi-input mfi-textarea"
      rows="3"
      @change="commitText"
    />

    <!-- Not v-model: on a type="number" input Vue casts the draft to a Number,
         and the commit below works on the text the user actually typed. -->
    <input
      v-else-if="field.kind === 'number'"
      :id="id"
      :value="draft"
      class="mfi-input"
      type="number"
      :min="field.min"
      :max="field.max"
      :step="field.step"
      @input="draft = ($event.target as HTMLInputElement).value"
      @change="commitNumber"
      @keydown.enter="commitNumber"
    />

    <label v-else-if="field.kind === 'toggle'" class="mfi-toggle">
      <input
        :id="id"
        type="checkbox"
        :checked="modelValue === true"
        @change="emit('update:modelValue', ($event.target as HTMLInputElement).checked)"
      />
      <span class="mfi-track" />
      <span class="mfi-toggle-note">{{ field.note ?? '' }}</span>
    </label>

    <input
      v-else
      :id="id"
      v-model="draft"
      class="mfi-input"
      type="text"
      @change="commitText"
      @keydown.enter="commitText"
    />

    <span v-if="field.note && field.kind !== 'toggle'" class="mfi-note">{{ field.note }}</span>
  </div>
</template>

<style scoped>
.mfi {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}
.mfi.wide { grid-column: 1 / -1; }
.mfi-label { font-size: 12px; font-weight: 600; color: var(--text-secondary); }
.mfi-input {
  width: 100%;
  padding: 7px 10px;
  font-size: 12.5px;
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  box-sizing: border-box;
}
.mfi-input:focus { outline: none; border-color: var(--accent); }
.mfi-textarea { resize: vertical; line-height: 1.5; }
.mfi-note { font-size: 11px; color: var(--text-tertiary); line-height: 1.5; }

.mfi-toggle { display: inline-flex; align-items: center; gap: 8px; cursor: pointer; }
.mfi-toggle input { display: none; }
.mfi-track {
  width: 32px;
  height: 18px;
  flex-shrink: 0;
  background: var(--border-default);
  border-radius: 9px;
  position: relative;
  transition: background 0.15s;
}
.mfi-track::after {
  content: '';
  position: absolute;
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: #fff;
  top: 3px;
  left: 3px;
  transition: left 0.15s;
}
.mfi-toggle input:checked + .mfi-track { background: var(--accent); }
.mfi-toggle input:checked + .mfi-track::after { left: 17px; }
.mfi-toggle-note { font-size: 12px; color: var(--text-secondary); }
</style>
