<script lang="ts">
  // Root of the Steam Deck edition (`HYPER_EDITION=deck`, see
  // vite.config.ts): console mode only. The desktop screens are not imported,
  // so they are not even in the bundle. Boot mirrors App.svelte.
  import { onMount } from 'svelte'
  import { settings, loadSettings } from '$lib/state/settings.svelte'
  import { loadLibrary, subscribeLibraryEvents } from '$lib/state/library.svelte'
  import { subscribeCloseRequests } from '$lib/state/close.svelte'
  import { setLocale, i18n } from '$lib/i18n/index.svelte'
  import { setFullscreen } from '$lib/api/commands'
  // no desktop fallback handler (sidebar, routes): console screens handle input
  import { startGamepadInput } from '$lib/gamepad/input.svelte'
  import Toasts from '$lib/components/Toasts.svelte'
  import ConfirmDialog from '$lib/components/ConfirmDialog.svelte'
  import TvosKeyboard from '$lib/components/TvosKeyboard.svelte'
  import { applyAppearance } from '$lib/theme'
  import Console from '$screens/Console/Console.svelte'

  let booted = $state(false)
  let bootError = $state<string | null>(null)

  onMount(() => {
    let unsub: (() => void) | undefined
    let unsubClose: (() => void) | undefined
    ;(async () => {
      try {
        const s = await loadSettings()
        await setLocale(s.language)
        startGamepadInput()
        unsub = await subscribeLibraryEvents()
        unsubClose = await subscribeCloseRequests()
        // Do not block the first paint on the library: it renders as it arrives.
        void loadLibrary()
        await setFullscreen(true).catch(() => {})
      } catch (e) {
        bootError = String(e)
      } finally {
        booted = true
      }
    })()
    return () => {
      unsub?.()
      unsubClose?.()
    }
  })

  $effect(() => {
    document.documentElement.style.setProperty('--titlebar-height', '0px')
  })

  // console mode is a TV UI and stays dark
  $effect(() => {
    applyAppearance(settings.value.appearance, settings.value.accentColor, true)
  })

  // Re-load locale when the user changes the language in Settings.
  $effect(() => {
    if (settings.loaded && settings.value.language !== i18n.locale) {
      setLocale(settings.value.language)
    }
  })
</script>

<Toasts />
<ConfirmDialog />
<TvosKeyboard />

{#if !booted}
  <div class="boot">
    <div class="spinner"></div>
  </div>
{:else if bootError}
  <div class="boot error">
    <h2>Hyper could not start</h2>
    <pre>{bootError}</pre>
  </div>
{:else}
  <Console />
{/if}

<style>
  .boot {
    height: 100%;
    display: grid;
    place-items: center;
    padding: var(--space-xl);
  }
  .boot.error {
    place-items: start;
    color: var(--danger);
  }
  .boot pre {
    white-space: pre-wrap;
    color: var(--text-secondary);
    user-select: text;
  }
  .spinner {
    width: 40px;
    height: 40px;
    border: 3px solid var(--background-lighter);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
