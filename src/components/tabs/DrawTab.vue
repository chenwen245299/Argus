<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { useCanvasStore, type DrawNodeSnapshot } from '../../stores/canvas'

const { t } = useI18n()
const canvasStore = useCanvasStore()

const node = computed(() => canvasStore.selectedNode)
const count = computed(() => canvasStore.selectedNodeIds.length)
const isShape = computed(() => node.value?.type === 'shape')
const isText = computed(() => node.value?.type === 'text')
const isLine = computed(() => node.value?.type === 'line')
const isImage = computed(() => node.value?.type === 'image')

function action(type: string, payload?: unknown) {
  canvasStore.requestAction(type, payload)
}

// Object-alignment buttons: [dir, title, svg inner markup]
const ALIGN_BTNS: { dir: string; title: string; inner: string }[] = [
  { dir: 'left', title: t('drawTab.alignLeft'), inner: '<line x1="4" y1="4" x2="4" y2="20" stroke="currentColor" stroke-width="2"/><rect x="7" y="6" width="13" height="4" rx="1"/><rect x="7" y="14" width="9" height="4" rx="1"/>' },
  { dir: 'hcenter', title: t('drawTab.alignHCenter'), inner: '<line x1="12" y1="3" x2="12" y2="21" stroke="currentColor" stroke-width="2"/><rect x="5" y="6" width="14" height="4" rx="1"/><rect x="8" y="14" width="8" height="4" rx="1"/>' },
  { dir: 'right', title: t('drawTab.alignRight'), inner: '<line x1="20" y1="4" x2="20" y2="20" stroke="currentColor" stroke-width="2"/><rect x="4" y="6" width="13" height="4" rx="1"/><rect x="8" y="14" width="9" height="4" rx="1"/>' },
  { dir: 'top', title: t('drawTab.alignTop'), inner: '<line x1="4" y1="4" x2="20" y2="4" stroke="currentColor" stroke-width="2"/><rect x="6" y="7" width="4" height="13" rx="1"/><rect x="14" y="7" width="4" height="9" rx="1"/>' },
  { dir: 'vcenter', title: t('drawTab.alignVCenter'), inner: '<line x1="3" y1="12" x2="21" y2="12" stroke="currentColor" stroke-width="2"/><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="8" width="4" height="8" rx="1"/>' },
  { dir: 'bottom', title: t('drawTab.alignBottom'), inner: '<line x1="4" y1="20" x2="20" y2="20" stroke="currentColor" stroke-width="2"/><rect x="6" y="4" width="4" height="13" rx="1"/><rect x="14" y="8" width="4" height="9" rx="1"/>' },
]

// Curated system / built-in font families.
const FONT_FAMILIES: { label: string; value: string }[] = [
  { label: t('drawTab.fontDefault'), value: '' },
  { label: 'PingFang SC', value: '"PingFang SC", sans-serif' },
  { label: 'Inter', value: 'Inter, sans-serif' },
  { label: 'Helvetica', value: 'Helvetica, Arial, sans-serif' },
  { label: 'Georgia', value: 'Georgia, serif' },
  { label: 'Times New Roman', value: '"Times New Roman", serif' },
  { label: 'Mono', value: 'ui-monospace, "SF Mono", Menlo, monospace' },
  { label: '楷体 KaiTi', value: 'KaiTi, STKaiti, serif' },
  { label: '宋体 SimSun', value: 'SimSun, "Songti SC", serif' },
  { label: '黑体 SimHei', value: 'SimHei, "Heiti SC", sans-serif' },
]

const SHAPE_KINDS: { value: 'rect' | 'ellipse' | 'diamond'; label: string }[] = [
  { value: 'rect', label: t('drawTab.shapeRect') },
  { value: 'ellipse', label: t('drawTab.shapeEllipse') },
  { value: 'diamond', label: t('drawTab.shapeDiamond') },
]

function patch(p: Partial<DrawNodeSnapshot>) {
  if (node.value) canvasStore.patchNode(node.value.nodeId, p)
}

function num(e: Event): number {
  return parseFloat((e.target as HTMLInputElement).value)
}

/** input[type=color] needs a #rrggbb value; fall back when unset / non-hex. */
function toHex(c: string | undefined, fallback: string): string {
  return c && /^#[0-9a-fA-F]{6}$/.test(c) ? c : fallback
}

const opacityPct = computed({
  get: () => Math.round((node.value?.opacity ?? 1) * 100),
  set: (v: number) => patch({ opacity: Math.min(1, Math.max(0, v / 100)) }),
})
</script>

<template>
  <div class="draw-tab">
    <div class="list-toolbar">
      <span class="list-heading">{{ t('drawTab.title') }}</span>
    </div>

    <!-- Multi-select batch panel -->
    <div v-if="count > 1" class="draw-body">
      <section class="prop-group">
        <h4 class="group-title">{{ t('drawTab.selected', { n: count }) }}</h4>
        <div class="seg-control">
          <button v-for="b in ALIGN_BTNS" :key="b.dir" class="seg-btn icon-btn-sq" :title="b.title" @click="action('align', b.dir)">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" v-html="b.inner" />
          </button>
        </div>
        <div class="field-pair">
          <button class="wide-btn" @click="action('distribute', 'h')" :disabled="count < 3">{{ t('drawTab.distributeH') }}</button>
          <button class="wide-btn" @click="action('distribute', 'v')" :disabled="count < 3">{{ t('drawTab.distributeV') }}</button>
        </div>
      </section>
      <section class="prop-group">
        <h4 class="group-title">{{ t('drawTab.batchColor') }}</h4>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.fill') }}</span>
          <input type="color" class="color-swatch" value="#ffffff" @input="action('batchPatch', { fillColor: ($event.target as HTMLInputElement).value })" />
        </label>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.stroke') }}</span>
          <input type="color" class="color-swatch" value="#1a1a1a" @input="action('batchPatch', { color: ($event.target as HTMLInputElement).value })" />
        </label>
      </section>
      <section class="prop-group">
        <h4 class="group-title">{{ t('drawTab.arrange') }}</h4>
        <div class="field-pair">
          <button class="wide-btn" @click="action('zorder', 'front')">{{ t('drawTab.front') }}</button>
          <button class="wide-btn" @click="action('zorder', 'back')">{{ t('drawTab.back') }}</button>
        </div>
        <div class="field-pair">
          <button class="wide-btn" @click="action('duplicate')">{{ t('drawTab.duplicate') }}</button>
          <button class="wide-btn danger" @click="action('delete')">{{ t('drawTab.delete') }}</button>
        </div>
      </section>
    </div>

    <!-- Empty state -->
    <div v-else-if="!node" class="draw-empty">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M12 19l7-7 3 3-7 7-3-3z"/><path d="M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z"/><path d="M2 2l7.586 7.586"/><circle cx="11" cy="11" r="2"/>
      </svg>
      <p>{{ t('drawTab.empty') }}</p>
      <span>{{ t('drawTab.emptyHint') }}</span>
    </div>

    <div v-else class="draw-body">
      <!-- Position -->
      <section class="prop-group">
        <h4 class="group-title">{{ t('drawTab.position') }}</h4>
        <div class="field-pair">
          <label class="mini-field">
            <span class="mini-label">X</span>
            <input type="number" :value="node.x" @change="patch({ x: num($event) })" />
          </label>
          <label class="mini-field">
            <span class="mini-label">Y</span>
            <input type="number" :value="node.y" @change="patch({ y: num($event) })" />
          </label>
        </div>
        <div v-if="isShape || isImage" class="field-pair">
          <label class="mini-field">
            <span class="mini-label">W</span>
            <input type="number" min="1" :value="node.width" @change="patch({ width: Math.max(1, num($event)) })" />
          </label>
          <label class="mini-field">
            <span class="mini-label">H</span>
            <input type="number" min="1" :value="node.height" @change="patch({ height: Math.max(1, num($event)) })" />
          </label>
        </div>
        <div v-if="isShape || isText || isImage" class="field-pair">
          <label class="mini-field">
            <span class="mini-label">{{ t('drawTab.rotation') }}</span>
            <input type="number" :value="node.rotation ?? 0" @change="patch({ rotation: num($event) })" />
          </label>
          <span class="mini-field-spacer" />
        </div>
      </section>

      <!-- Appearance -->
      <section v-if="isShape || isText || isImage" class="prop-group">
        <h4 class="group-title">{{ t('drawTab.appearance') }}</h4>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.opacity') }}</span>
          <div class="row-input-group">
            <input type="range" min="0" max="100" v-model.number="opacityPct" class="slider" />
            <input type="number" min="0" max="100" v-model.number="opacityPct" class="num-sm" />
          </div>
        </label>
        <label v-if="isShape || isImage" class="row-field">
          <span class="row-label">{{ t('drawTab.cornerRadius') }}</span>
          <input
            type="number" min="0"
            class="num-sm"
            :value="node.cornerRadius ?? 6"
            :disabled="isShape && (node.shapeKind === 'ellipse' || node.shapeKind === 'diamond')"
            @change="patch({ cornerRadius: Math.max(0, num($event)) })"
          />
        </label>
      </section>

      <!-- Shape -->
      <section v-if="isShape" class="prop-group">
        <h4 class="group-title">{{ t('drawTab.shape') }}</h4>
        <div class="seg-control">
          <button
            v-for="sk in SHAPE_KINDS"
            :key="sk.value"
            class="seg-btn"
            :class="{ active: (node.shapeKind ?? 'rect') === sk.value }"
            @click="patch({ shapeKind: sk.value })"
          >{{ sk.label }}</button>
        </div>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.fill') }}</span>
          <input type="color" class="color-swatch" :value="toHex(node.fillColor, '#ffffff')" @input="patch({ fillColor: ($event.target as HTMLInputElement).value })" />
        </label>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.stroke') }}</span>
          <input type="color" class="color-swatch" :value="toHex(node.color, '#1a1a1a')" @input="patch({ color: ($event.target as HTMLInputElement).value })" />
        </label>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.strokeWidth') }}</span>
          <input type="number" min="0" step="0.5" class="num-sm" :value="node.strokeWidth ?? 2" @change="patch({ strokeWidth: Math.max(0, num($event)) })" />
        </label>
      </section>

      <!-- Line / arrow -->
      <section v-if="isLine" class="prop-group">
        <h4 class="group-title">{{ t('drawTab.line') }}</h4>
        <div class="seg-control">
          <button class="seg-btn" :class="{ active: node.lineKind !== 'arrow' }" @click="patch({ lineKind: 'line' })">{{ t('drawTab.lineStraight') }}</button>
          <button class="seg-btn" :class="{ active: node.lineKind === 'arrow' }" @click="patch({ lineKind: 'arrow' })">{{ t('drawTab.lineArrow') }}</button>
        </div>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.stroke') }}</span>
          <input type="color" class="color-swatch" :value="toHex(node.color, '#1a1a1a')" @input="patch({ color: ($event.target as HTMLInputElement).value })" />
        </label>
        <label class="row-field">
          <span class="row-label">{{ t('drawTab.strokeWidth') }}</span>
          <input type="number" min="0.5" step="0.5" class="num-sm" :value="node.strokeWidth ?? 2" @change="patch({ strokeWidth: Math.max(0.5, num($event)) })" />
        </label>
      </section>

      <!-- Typography -->
      <section v-if="isText" class="prop-group">
        <h4 class="group-title">{{ t('drawTab.typography') }}</h4>
        <label class="col-field">
          <span class="row-label">{{ t('drawTab.font') }}</span>
          <select class="full-select" :value="node.fontFamily ?? ''" @change="patch({ fontFamily: ($event.target as HTMLSelectElement).value })">
            <option v-for="f in FONT_FAMILIES" :key="f.label" :value="f.value">{{ f.label }}</option>
          </select>
        </label>
        <div class="field-pair">
          <label class="mini-field">
            <span class="mini-label">{{ t('drawTab.fontSize') }}</span>
            <input type="number" min="1" max="800" :value="node.fontSize ?? 14" @change="patch({ fontSize: Math.max(1, num($event)) })" />
          </label>
          <div class="style-toggles">
            <button class="toggle-btn" :class="{ active: node.bold }" @click="patch({ bold: !node.bold })" style="font-weight:700">B</button>
            <button class="toggle-btn" :class="{ active: node.italic }" @click="patch({ italic: !node.italic })" style="font-style:italic">I</button>
          </div>
        </div>
        <div class="field-pair">
          <div class="seg-control align-seg">
            <button class="seg-btn" :class="{ active: (node.textAlign ?? 'left') === 'left' }" :title="t('drawTab.alignLeft')" @click="patch({ textAlign: 'left' })">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M21 6H3"/><path d="M15 12H3"/><path d="M17 18H3"/></svg>
            </button>
            <button class="seg-btn" :class="{ active: node.textAlign === 'center' }" :title="t('drawTab.alignHCenter')" @click="patch({ textAlign: 'center' })">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M21 6H3"/><path d="M17 12H7"/><path d="M19 18H5"/></svg>
            </button>
            <button class="seg-btn" :class="{ active: node.textAlign === 'right' }" :title="t('drawTab.alignRight')" @click="patch({ textAlign: 'right' })">
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M21 6H3"/><path d="M21 12H9"/><path d="M21 18H7"/></svg>
            </button>
          </div>
          <label class="mini-field color-field">
            <span class="mini-label">{{ t('drawTab.textColor') }}</span>
            <input type="color" class="color-swatch" :value="toHex(node.color, '#1a1a1a')" @input="patch({ color: ($event.target as HTMLInputElement).value })" />
          </label>
        </div>
      </section>

      <!-- Arrange (single selection) -->
      <section class="prop-group">
        <h4 class="group-title">{{ t('drawTab.arrange') }}</h4>
        <div class="field-pair">
          <button class="wide-btn" @click="action('zorder', 'front')">{{ t('drawTab.front') }}</button>
          <button class="wide-btn" @click="action('zorder', 'back')">{{ t('drawTab.back') }}</button>
        </div>
        <div class="field-pair">
          <button class="wide-btn" @click="action('duplicate')">{{ t('drawTab.duplicate') }}</button>
          <button class="wide-btn danger" @click="action('delete')">{{ t('drawTab.delete') }}</button>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.draw-tab { height: 100%; display: flex; flex-direction: column; overflow: hidden; }
.list-toolbar {
  display: flex; align-items: center; height: 40px; padding: 0 12px;
  background: var(--bg-secondary); border-bottom: 1px solid var(--border-subtle); flex-shrink: 0;
}
.list-heading { flex: 1; font-size: var(--font-size-sm); font-weight: 600; color: var(--text-primary); }

.draw-empty {
  flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center;
  gap: 8px; color: var(--text-tertiary); padding: 24px; text-align: center;
}
.draw-empty p { font-size: var(--font-size-sm); color: var(--text-secondary); }
.draw-empty span { font-size: var(--font-size-xs); }

.draw-body { flex: 1; overflow-y: auto; padding: 4px 0 16px; }

.prop-group { padding: 12px 14px; border-bottom: 1px solid var(--border-subtle); display: flex; flex-direction: column; gap: 9px; }
.group-title { font-size: var(--font-size-xs); font-weight: 600; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.04em; }

.field-pair { display: flex; gap: 8px; }
.mini-field { flex: 1; display: flex; align-items: center; gap: 6px; background: var(--bg-secondary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); padding: 3px 8px; min-width: 0; }
.mini-field-spacer { flex: 1; }
.mini-label { font-size: 11px; color: var(--text-tertiary); flex-shrink: 0; }
.mini-field input { flex: 1; min-width: 0; border: none; background: transparent; color: var(--text-primary); font-size: var(--font-size-sm); }
.mini-field input:focus { outline: none; }
.color-field { flex: 0 0 auto; }

.row-field { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.col-field { display: flex; flex-direction: column; gap: 5px; }
.row-label { font-size: var(--font-size-sm); color: var(--text-secondary); }
.row-input-group { display: flex; align-items: center; gap: 8px; }
.slider { width: 96px; accent-color: var(--accent); }
.num-sm { width: 56px; padding: 4px 6px; font-size: var(--font-size-sm); text-align: right; border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-secondary); color: var(--text-primary); }
.num-sm:focus { outline: none; border-color: var(--accent); }
.num-sm:disabled { opacity: 0.45; }

.full-select { width: 100%; padding: 5px 8px; font-size: var(--font-size-sm); border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-secondary); color: var(--text-primary); cursor: pointer; }
.full-select:focus { outline: none; border-color: var(--accent); }

.seg-control { display: flex; gap: 4px; background: var(--bg-secondary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); padding: 2px; }
.align-seg { flex: 1; }
.seg-btn { flex: 1; display: inline-flex; align-items: center; justify-content: center; padding: 4px 8px; font-size: var(--font-size-xs); color: var(--text-secondary); border-radius: calc(var(--radius-sm) - 2px); transition: background 0.12s, color 0.12s; }
.seg-btn svg { display: block; }
.seg-btn:hover { background: var(--bg-hover); }
.seg-btn.active { background: var(--accent); color: #fff; }

.color-swatch { width: 30px; height: 24px; padding: 0; border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: none; cursor: pointer; }

.wide-btn { flex: 1; padding: 6px 8px; font-size: var(--font-size-sm); color: var(--text-secondary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-secondary); transition: background 0.12s, color 0.12s; }
.wide-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
.wide-btn:disabled { opacity: 0.4; cursor: not-allowed; }
.wide-btn.danger { color: #cc3333; }
.wide-btn.danger:hover { background: color-mix(in srgb, #cc3333 12%, var(--bg-secondary)); }

.style-toggles { display: flex; gap: 6px; }
.toggle-btn { width: 30px; height: 30px; font-size: 14px; color: var(--text-secondary); border: 1px solid var(--border-default); border-radius: var(--radius-sm); transition: background 0.12s, color 0.12s; }
.toggle-btn:hover { background: var(--bg-hover); }
.toggle-btn.active { background: var(--accent); color: #fff; border-color: var(--accent); }
</style>
