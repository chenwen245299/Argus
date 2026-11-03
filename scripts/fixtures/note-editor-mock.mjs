import { defineComponent, h, ref } from 'vue'

// Small synchronous editor for lifecycle tests; the Markdown feature suite uses real Vditor.
export default defineComponent({
  props: ['initialContent'],
  emits: ['change'],
  setup(props, { emit, expose }) {
    const text = ref(props.initialContent)
    expose({ setContent(value) { text.value = value; return true }, getContent: () => text.value })
    return () => h('textarea', {
      class: 'mock-editor', value: text.value,
      onInput(event) { text.value = event.target.value; emit('change', text.value) },
      style: 'width:100%;height:100%;box-sizing:border-box;background:transparent;color:inherit;padding:16px;resize:none;border:0',
    })
  },
})
