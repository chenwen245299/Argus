import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [vue()],
  // Browser regressions use Vite's default cache with different entry points
  // and desktop mocks. They must not replace the app's optimized dependencies.
  cacheDir: 'node_modules/.vite-argus-dev',
  clearScreen: false,
  server: {
    host: host || false,
    port: 1420,
    strictPort: true,
    // A Tauri WebView keeps its HTTP cache across launches. After a forced
    // optimization, cached dependency modules can refer to removed chunks even
    // when Vite's version query is unchanged. Keep the disk cache, not HTTP copies.
    headers: { 'Cache-Control': 'no-store' },
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
