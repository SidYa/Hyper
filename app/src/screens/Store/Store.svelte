<script lang="ts">
  // The Store tab. The web store itself is a native child webview managed
  // by Rust (commands/storeview.rs); this component tells the backend where
  // the webview should sit. There is no toolbar: stores are switched from the
  // sidebar.
  import {
    errorMessage,
    storeWebviewBounds,
    storeWebviewCommand,
    storeWebviewHide,
    storeWebviewReleaseFocus,
    storeWebviewScrollbar,
    storeWebviewShow,
    type StoreWebviewCommand
  } from '$lib/api/commands'
  import { getCurrentWindow } from '@tauri-apps/api/window'
  import { notify } from '$lib/state/toast.svelte'
  import { gamepad, pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { currentFocus } from '$lib/gamepad/spatial'
  import { RUNNERS, type StoreRunner } from '$types/index'

  // Gamepad: once "armed" every press is forwarded to the page's cursor
  // script; B hands control back to the sidebar.
  let armed = $state(false)
  function cmd(c: StoreWebviewCommand) {
    storeWebviewCommand(active, c).catch(() => {})
  }
  function arm() {
    armed = true
    ;(document.activeElement as HTMLElement | null)?.blur()
    cmd({ type: 'enter' })
  }
  function disarm() {
    armed = false
    cmd({ type: 'exit' })
    const target = document.querySelector<HTMLElement>('.sidebar .store.active')
    target?.focus()
  }
  // When the app window comes back to the foreground the store page may own
  // focus, which blinds the Gamepad API; give it back to the UI.
  $effect(() => {
    let unlisten: (() => void) | undefined
    let gone = false
    getCurrentWindow()
      .onFocusChanged(({ payload }) => {
        if (payload && gamepad.usingGamepad) storeWebviewReleaseFocus().catch(() => {})
      })
      .then((u) => {
        // unmounted before the listener was registered: drop it at once
        if (gone) u()
        else unlisten = u
      })
      .catch(() => {})
    return () => {
      gone = true
      unlisten?.()
    }
  })

  $effect(() => {
    void active
    if (gamepad.usingGamepad) arm()
    return pushHandler((action) => {
      const dir = directionOf(action)
      if (!armed) {
        const el = currentFocus()
        const insideView = !el || (container?.contains(el) ?? false)
        if ((dir || action === 'mainAction') && insideView) {
          arm()
          return true
        }
        return false
      }
      if (dir) {
        cmd({ type: 'navigate', direction: dir })
        return true
      }
      switch (action) {
        case 'mainAction':
          cmd({ type: 'click' })
          return true
        case 'back':
          disarm()
          return true
        case 'altAction':
          cmd({ type: 'focusSearch' })
          return true
        case 'prevPage':
          cmd({ type: 'goBack' })
          return true
        case 'nextPage':
          cmd({ type: 'goForward' })
          return true
        case 'rightStickUp':
          cmd({ type: 'scroll', direction: 'up' })
          return true
        case 'rightStickDown':
          cmd({ type: 'scroll', direction: 'down' })
          return true
        case 'guide':
          return false
        default:
          return true
      }
    })
  })

  interface Props {
    runner?: string
  }
  let { runner }: Props = $props()

  const active = $derived<StoreRunner>(RUNNERS.includes(runner as StoreRunner) ? (runner as StoreRunner) : 'legendary')

  let container = $state<HTMLDivElement | undefined>()
  let ready = $state(false)

  // show/hide are async IPC calls; run them strictly in order so a late
  // "hide" from the previous store cannot land after the next "show".
  let chain: Promise<unknown> = Promise.resolve()
  function queued<T>(fn: () => Promise<T>): Promise<T> {
    const next = chain.then(fn, fn)
    chain = next.catch(() => {})
    return next
  }

  function bounds() {
    const r = container!.getBoundingClientRect()
    return { x: r.left, y: r.top, width: r.width, height: r.height }
  }

  // The store pages get the Library's scrollbar in the current theme's
  // colours (the same variables app.css uses).
  function syncScrollbar() {
    const css = getComputedStyle(document.documentElement)
    const thumb = css.getPropertyValue('--background-lighter').trim()
    const hover = css.getPropertyValue('--text-tertiary').trim()
    if (thumb && hover) storeWebviewScrollbar(thumb, hover).catch(() => {})
  }

  // (Re)attach the native webview whenever the store or the layout changes.
  $effect(() => {
    const store = active
    const el = container
    if (!el) return
    ready = false
    let cancelled = false
    syncScrollbar()
    queued(() => storeWebviewShow(store, bounds()))
      .then(() => {
        if (cancelled) return
        ready = true
      })
      .catch((e) => notify(errorMessage(e), 'error'))

    const ro = new ResizeObserver(() => {
      storeWebviewBounds(store, bounds()).catch(() => {})
    })
    ro.observe(el)
    const onResize = () => storeWebviewBounds(store, bounds()).catch(() => {})
    window.addEventListener('resize', onResize)

    return () => {
      cancelled = true
      ro.disconnect()
      window.removeEventListener('resize', onResize)
      queued(() => storeWebviewHide(store)).catch(() => {})
    }
  })
</script>

<div class="store">
  <div class="view" bind:this={container}>
    {#if !ready}<div class="muted loading">…</div>{/if}
  </div>
</div>

<style>
  .store {
    display: flex;
    flex-direction: column;
    height: 100%;
    /* the store is a native webview on top of the page: keep it below the
       custom title bar's window buttons */
    padding-top: var(--titlebar-height, 0px);
  }
  .view {
    flex: 1;
    min-height: 0;
    position: relative;
    background: var(--background-darker);
  }
  .loading {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 24px;
  }
</style>
