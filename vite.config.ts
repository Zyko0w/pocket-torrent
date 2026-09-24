/// <reference types="vitest/config" />
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://v2.tauri.app/start/frontend/vite/
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // seule notre page : sinon Vite explore aussi l'ancienne interface web du moteur vendorisé
  // (src-tauri/vendor/librqbit/webui) et réclame ses dépendances
  optimizeDeps: { entries: ['index.html'] },
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] }
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    target: 'chrome120',
    minify: 'esbuild',
    sourcemap: false
  },
  // tests de l'interface (npm test) : navigateur simulé, sans Tauri ni moteur
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    restoreMocks: true
  }
})
