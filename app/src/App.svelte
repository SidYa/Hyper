<script lang="ts">
  import { onMount } from 'svelte'
  import { router, navigate } from '$lib/router.svelte'
  import { settings, loadSettings } from '$lib/state/settings.svelte'
  import { loadLibrary, subscribeLibraryEvents } from '$lib/state/library.svelte'
  import { subscribeCloseRequests } from '$lib/state/close.svelte'
  import { setLocale, i18n } from '$lib/i18n/index.svelte'
  import { setFullscreen } from '$lib/api/commands'
  import { hostReady } from '$lib/state/host.svelte'
  import { setFallbackHandler, startGamepadInput } from '$lib/gamepad/input.svelte'
  import { defaultHandler } from '$lib/gamepad/navigation'
  import Sidebar from '$lib/components/Sidebar.svelte'
  import Toasts from '$lib/components/Toasts.svelte'
  import ConfirmDialog from '$lib/components/ConfirmDialog.svelte'
  import WindowControls from '$lib/components/WindowControls.svelte'
  import TvosKeyboard from '$lib/components/TvosKeyboard.svelte'
  import { applyAppearance } from '$lib/theme'
  import Library from '$screens/Library/Library.svelte'
  import Console from '$screens/Console/Console.svelte'
  import Game from '$screens/Game/Game.svelte'
  import Settings from '$screens/Settings/Settings.svelte'
  import Downloads from '$screens/Downloads/Downloads.svelte'
  import Store from '$screens/Store/Store.svelte'

  let booted = $state(false)
  let bootError = $state<string | null>(null)

  const route = $derived(router.route)
  const isConsole = $derived(route.name === 'console')
  // own title bar in desktop mode (console mode is fullscreen anyway)
  const customTitlebar = $derived(!isConsole)

  onMount(() => {
    let unsub: (() => void) | undefined
    let unsubClose: (() => void) | undefined
    ;(async () => {
      try {
        const [info, s] = await Promise.all([hostReady, loadSettings()])
        await setLocale(s.language)
        setFallbackHandler(defaultHandler)
        startGamepadInput()
        unsub = await subscribeLibraryEvents()
        unsubClose = await subscribeCloseRequests()
        // Do not block the first paint on the library: it renders as it arrives.
        void loadLibrary()
        // Same rule as the original: CLI flag or setting opens console mode.
        if ((info.startInConsoleCli || s.startInConsoleMode) && route.name === 'library') {
          navigate('/console')
        }
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

  // Console mode owns the whole screen; leaving it restores the window.
  $effect(() => {
    setFullscreen(isConsole).catch(() => {})
  })

  // Title bar: the window has no system frame (lib.rs); reserve room for our
  // window buttons.
  $effect(() => {
    document.documentElement.style.setProperty('--titlebar-height', customTitlebar ? '32px' : '0px')
  })

  // Theme + accent; console mode is a TV UI and stays dark.
  $effect(() => {
    applyAppearance(settings.value.appearance, settings.value.accentColor, isConsole)
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
{:else if isConsole}
  <Console />
{:else}
  <div class="shell" class:titlebar={customTitlebar}>
    {#if customTitlebar}
      <!-- drag strip over the top of the page; on the game page it starts
           right of the back button -->
      <div class="drag" class:game={route.name === 'game'} data-tauri-drag-region></div>
      <WindowControls />
    {/if}
    <Sidebar />
    <main class="content" class:bleed={route.name === 'game' || route.name === 'store'}>
      {#if route.name === 'library'}
        <Library />
      {:else if route.name === 'game'}
        <Game runner={route.params.runner} appName={route.params.appName} />
      {:else if route.name === 'settings'}
        <Settings section={route.params.section} />
      {:else if route.name === 'downloads'}
        <Downloads />
      {:else if route.name === 'store'}
        <Store runner={route.params.runner} />
      {/if}
    </main>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: var(--sidebar-width) 1fr;
    height: 100%;
  }
  .content {
    min-width: 0;
    overflow: auto;
    background: var(--body-background);
  }
  /* clear the window buttons; full-bleed pages handle the top themselves */
  .shell.titlebar .content:not(.bleed) {
    padding-top: 12px;
  }
  /* the scrollbar starts below the window buttons instead of under Close */
  .shell.titlebar .content::-webkit-scrollbar-track {
    margin-top: 32px;
  }
  .drag {
    position: fixed;
    top: 0;
    left: var(--sidebar-width);
    right: 138px;
    height: var(--titlebar-height);
    z-index: 800;
  }
  .drag.game {
    left: calc(var(--sidebar-width) + 64px);
  }
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
