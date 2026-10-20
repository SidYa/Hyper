<script lang="ts">
  // Minimise / maximise / close for the custom title bar, in a floating
  // frosted pill. Drawn in the theme: accent on hover, the danger colour
  // for close.
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { t } from '$lib/i18n/index.svelte'

  const win = getCurrentWindow()
  let maximized = $state(false)

  $effect(() => {
    let off: (() => void) | undefined
    let gone = false
    const sync = () => win.isMaximized().then((m) => (maximized = m)).catch(() => {})
    void sync()
    win
      .onResized(sync)
      .then((u) => {
        // unmounted before the listener was registered: drop it at once
        if (gone) u()
        else off = u
      })
      .catch(() => {})
    return () => {
      gone = true
      off?.()
    }
  })
</script>

<div class="controls">
  <button onclick={() => win.minimize()} aria-label={t('window.minimize', 'Minimize')} title={t('window.minimize', 'Minimize')}>
    <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M1.5 6h9" /></svg>
  </button>
  <button
    onclick={() => win.toggleMaximize()}
    aria-label={maximized ? t('window.restore', 'Restore') : t('window.maximize', 'Maximize')}
    title={maximized ? t('window.restore', 'Restore') : t('window.maximize', 'Maximize')}
  >
    {#if maximized}
      <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M3.5 3.5V1.5h7v7h-2M1.5 3.5h7v7h-7z" /></svg>
    {:else}
      <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M1.5 1.5h9v9h-9z" /></svg>
    {/if}
  </button>
  <button class="close" onclick={() => win.close()} aria-label={t('window.close', 'Close')} title={t('window.close', 'Close')}>
    <svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2l8 8M10 2l-8 8" /></svg>
  </button>
</div>

<style>
  .controls {
    /* floating pill inside the title strip: off the window edges, so the
       buttons never stick to the top; must stay within --titlebar-height
       (the Store's native webview starts right below it) */
    position: fixed;
    top: 4px;
    right: 6px;
    z-index: 900;
    display: flex;
    gap: 2px;
    padding: 2px;
    /* own frosted backing: the page scrolls under the buttons, and over
       bright or grayscale covers bare glyphs disappear */
    background: color-mix(in srgb, var(--body-background) 72%, transparent);
    backdrop-filter: blur(14px) saturate(1.2);
    border: 1px solid color-mix(in srgb, var(--text-default) 10%, transparent);
    border-radius: 10px;
    box-shadow: 0 2px 12px rgba(0, 0, 0, 0.25);
  }
  button {
    width: 34px;
    height: 20px;
    display: grid;
    place-items: center;
    color: var(--text-default);
    border-radius: 7px;
    transition: background 120ms ease, color 120ms ease;
  }
  button:hover {
    background: color-mix(in srgb, var(--accent) 85%, transparent);
    color: var(--accent-text);
  }
  button:active {
    background: var(--accent);
  }
  .close:hover,
  .close:active {
    background: var(--danger);
    color: #fff;
  }
  svg {
    width: 11px;
    height: 11px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
</style>
