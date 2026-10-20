import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { fileURLToPath, URL } from 'node:url'

const host = process.env.TAURI_DEV_HOST
// `deck`: the Steam Deck edition, console mode only (src/DeckApp.svelte). It
// is built together with the Rust feature `deck` (scripts/build-linux.sh).
const deck = process.env.HYPER_EDITION === 'deck'

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],
  resolve: {
    alias: {
      $lib: fileURLToPath(new URL('./src/lib', import.meta.url)),
      $screens: fileURLToPath(new URL('./src/screens', import.meta.url)),
      $types: fileURLToPath(new URL('./src/types', import.meta.url)),
      $root: fileURLToPath(new URL(deck ? './src/DeckApp.svelte' : './src/App.svelte', import.meta.url))
    }
  },
  // Vite options tailored for Tauri development
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: {
      // Tell Vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**']
    }
  },
  build: {
    outDir: 'build',
    target: ['es2022', 'chrome105'],
    sourcemap: !!process.env.TAURI_ENV_DEBUG
  }
})
