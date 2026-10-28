import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// The renderer never knows the runtime origin: in dev, Vite proxies the two
// surfaces the UI is allowed to touch onto the local `arkd` listener.
const RUNTIME_PORT = process.env.ARK_RUNTIME_PORT ?? '18790'
const RUNTIME_ORIGIN = `http://127.0.0.1:${RUNTIME_PORT}`
const DEV_PORT = 5273

export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: DEV_PORT,
    strictPort: true,
    proxy: {
      // SSE must not be buffered: http-proxy pipes the response straight through.
      '/v1': { target: RUNTIME_ORIGIN, changeOrigin: true },
      '/health': { target: RUNTIME_ORIGIN, changeOrigin: true }
    }
  },
  css: {
    // Declare PostCSS explicitly. Without this Vite walks up to the parent
    // checkout's postcss.config.js (tailwind), which this project must not
    // inherit: the web renderer ships hand-written CSS only.
    postcss: { plugins: [] }
  },
  build: {
    outDir: 'dist',
    sourcemap: true,
    target: 'es2022'
  }
})
