import { ref, computed } from 'vue'
import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import type { CliOutputPolish, CliSettings, CliTool, CliPromptTemplate } from '../types'

const DEFAULT_POLISH: CliOutputPolish = {
  enabled: false,
  provider_id: '',
  model_id: '',
  prompt: "以下是 CLI 工具的原始输出内容。请将其转换为整洁的 Markdown 格式。保持所有内容完全一致，不要新增、删除或修改任何信息。只进行格式优化：代码片段用正确语言标签的围栏代码块包裹，数学公式用 LaTeX 格式表示（行内用 $...$，独立公式用 $$...$$）。只输出格式化后的 Markdown，不要输出其他任何内容。",
}

export const useCliStore = defineStore('cli', () => {
  const settings = ref<CliSettings>({ tools: [], prompt_templates: [], polish: { ...DEFAULT_POLISH } })

  const enabledTools = computed(() =>
    settings.value.tools.filter(t => t.enabled)
  )

  async function load() {
    try {
      const s = await invoke<CliSettings>('get_cli_settings')
      settings.value = { ...s, polish: s.polish ?? { ...DEFAULT_POLISH } }
    } catch {
      settings.value = { tools: [], prompt_templates: [], polish: { ...DEFAULT_POLISH } }
    }
  }

  async function detect() {
    const tools = await invoke<CliTool[]>('detect_cli_tools')
    await load()
    return tools
  }

  async function saveTool(tool: CliTool) {
    await invoke('save_cli_tool', { tool })
    await load()
  }

  async function deleteTool(id: string) {
    await invoke('delete_cli_tool', { id })
    await load()
  }

  async function testTool(id: string): Promise<string> {
    return invoke<string>('test_cli_tool', { id })
  }

  async function loadTemplates(): Promise<CliPromptTemplate[]> {
    return invoke<CliPromptTemplate[]>('get_cli_prompt_templates')
  }

  async function saveTemplate(template: CliPromptTemplate) {
    await invoke('save_cli_prompt_template', { template })
  }

  async function deleteTemplate(id: string) {
    await invoke('delete_cli_prompt_template', { id })
  }

  async function savePolish(polish: CliOutputPolish) {
    await invoke('save_cli_polish', { polish })
    await load()
  }

  return {
    settings,
    enabledTools,
    load,
    detect,
    saveTool,
    deleteTool,
    testTool,
    loadTemplates,
    saveTemplate,
    deleteTemplate,
    savePolish,
  }
})
