import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [vue()],
  clearScreen: false,
  server: {
    host: host || false,
    port: 1420,
    strictPort: true,
    hmr: host
      ? { protocol: 'ws', host, port: 1421 }
      : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  optimizeDeps: {
    include: [
      'pdfjs-dist',
      'highlight.js/lib/common',
      'katex',
      'dompurify',
      'marked',
      'llamaindex',
    ],
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          // llamaindex + its tokenizer are large — isolate them for lazy loading.
          'llamaindex-vendor': ['llamaindex'],
        },
      },
    },
  },
})
