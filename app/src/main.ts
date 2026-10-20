import { mount } from 'svelte'
import './styles/app.css'
// App.svelte, or DeckApp.svelte in the Steam Deck edition (vite.config.ts)
import App from '$root'

// A desktop app, not a web page: no browser context menu (Back, Reload,
// Print, Inspect…) and no DevTools shortcuts. Text fields keep the native
// menu so Cut / Copy / Paste still work there, and selected text can be copied.
const devtools = import.meta.env.DEV && import.meta.env.VITE_DEVTOOLS === '1'
if (!devtools) {
  window.addEventListener('contextmenu', (e) => {
    const el = e.target as HTMLElement | null
    const editable = !!el?.closest('input, textarea, [contenteditable="true"]')
    const hasSelection = !!window.getSelection()?.toString()
    if (!editable && !hasSelection) e.preventDefault()
  })
  window.addEventListener(
    'keydown',
    (e) => {
      const k = e.key.toUpperCase()
      const devKey =
        e.key === 'F12' ||
        (e.ctrlKey && e.shiftKey && ['I', 'J', 'C'].includes(k)) ||
        (e.ctrlKey && k === 'U')
      // also stop browser reload / print, which would reset or dump the UI
      const browserKey = e.key === 'F5' || (e.ctrlKey && (k === 'R' || k === 'P'))
      if (devKey || browserKey) e.preventDefault()
    },
    true
  )
}

const app = mount(App, { target: document.getElementById('app')! })

export default app
