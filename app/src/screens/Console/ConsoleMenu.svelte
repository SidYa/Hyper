<script lang="ts" module>
  export type ConsolePage = 'library' | 'downloads' | 'settings'
  export type MenuItemId = ConsolePage | 'exitConsole' | 'quitApp'
</script>

<script lang="ts">
  // Side menu for console mode, in the spirit of Steam Big Picture: slides in
  // from the left over the dimmed page. Opened with B / Start / Guide from the
  // library, closed with B, Right or a pick.
  import { onMount } from 'svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { gamepad, pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { library } from '$lib/state/library.svelte'
  import { host } from '$lib/state/host.svelte'
  import Brand from '$lib/components/Brand.svelte'
  import ControllerHints from '$lib/components/ControllerHints.svelte'
  import ConsoleClock from './ConsoleClock.svelte'

  interface Props {
    current: ConsolePage
    onselect: (id: MenuItemId) => void
    onclose: () => void
  }
  let { current, onselect, onclose }: Props = $props()

  // Icons are 24×24 strokes in currentColor (same style as the button glyphs).
  const ICONS: Record<MenuItemId, string> = {
    library: 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z',
    downloads: 'M12 3v11m-4.5-4.5L12 14l4.5-4.5M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3',
    settings: 'M4 7h9m4 0h3M4 17h3m4 0h9M15 4.5v5M9 14.5v5',
    exitConsole: 'M3 5h18v11H3zM8 20h8M12 16v4',
    quitApp: 'M12 3v8M7.1 6.3a7.5 7.5 0 1 0 9.8 0'
  }

  // Linux (Steam Deck) stays in console mode: no way out to the desktop UI.
  const alwaysConsole = $derived(host.alwaysConsole)

  const items = $derived<Array<{ id: MenuItemId; label: string; badge?: number; gap?: boolean }>>([
    { id: 'library', label: t('console.menu.library', 'Library') },
    { id: 'downloads', label: t('console.menu.downloads', 'Downloads'), badge: library.downloads.length || undefined },
    { id: 'settings', label: t('console.menu.settings', 'Settings') },
    ...(alwaysConsole ? [] : [{ id: 'exitConsole' as const, label: t('console.menu.exitConsole', 'Exit console mode'), gap: true }]),
    { id: 'quitApp', label: t('console.menu.quitApp', 'Quit Hyper'), gap: alwaysConsole }
  ])

  let index = $state(0)
  onMount(() => {
    // open on the page we came from
    index = Math.max(0, items.findIndex((i) => i.id === current))
  })

  $effect(() =>
    pushHandler((action) => {
      const dir = directionOf(action)
      if (dir === 'up') index = (index - 1 + items.length) % items.length
      else if (dir === 'down') index = (index + 1) % items.length
      else if (action === 'mainAction') onselect(items[index].id)
      else if (action === 'back' || action === 'start' || action === 'guide' || dir === 'right') onclose()
      return true
    })
  )

  const hints = $derived([
    { buttons: ['main' as const], label: t('console.select', 'Select') },
    { buttons: ['back' as const], label: t('console.menu.close', 'Close') }
  ])
</script>

<div class="scrim" onclick={onclose} role="presentation"></div>
<nav class="menu" aria-label={t('console.menu.title', 'Menu')}>
  <header>
    <Brand size={24} />
    <ConsoleClock />
  </header>

  <ul>
    {#each items as item, i (item.id)}
      <li class:gap={item.gap}>
        <button
          class="item"
          class:focused={i === index}
          class:current={item.id === current}
          onmouseenter={() => (index = i)}
          onclick={() => onselect(item.id)}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d={ICONS[item.id]} /></svg>
          <span class="label">{item.label}</span>
          {#if item.badge}<span class="badge">{item.badge}</span>{/if}
        </button>
      </li>
    {/each}
  </ul>

  {#if gamepad.connected}
    <footer><ControllerHints layout={gamepad.layout} {hints} /></footer>
  {/if}
</nav>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(6px);
    animation: fade 180ms ease-out;
  }
  .menu {
    position: fixed;
    inset: 0 auto 0 0;
    z-index: 41;
    width: min(400px, 86vw);
    display: flex;
    flex-direction: column;
    padding: 36px 0 24px;
    background: color-mix(in srgb, var(--background-darker) 94%, transparent);
    border-right: 1px solid rgba(255, 255, 255, 0.08);
    box-shadow: 24px 0 60px rgba(0, 0, 0, 0.5);
    animation: slide 200ms cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 36px;
  }
  ul {
    list-style: none;
    margin: 28px 0 0;
    padding: 0 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
  }
  li.gap {
    margin-top: 20px;
    padding-top: 20px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }
  .item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 18px;
    height: 58px;
    padding: 0 20px;
    border-radius: 10px;
    font-size: 21px;
    font-weight: 500;
    color: var(--text-secondary);
    text-align: left;
    outline: none;
    transition: background 120ms ease, color 120ms ease;
  }
  .item.current {
    color: var(--text-default);
  }
  .item.focused {
    background: var(--text-default);
    color: var(--background-darker);
  }
  svg {
    width: 26px;
    height: 26px;
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .label {
    flex: 1;
  }
  .badge {
    min-width: 28px;
    padding: 2px 9px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--accent-text);
    font-size: 15px;
    font-weight: 600;
    text-align: center;
  }
  footer {
    padding: 0 24px;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
