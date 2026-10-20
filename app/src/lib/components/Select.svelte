<script lang="ts" generics="T extends string">
  // Themed dropdown that replaces the native <select> (whose popup cannot be
  // styled in WebView2). Mouse, keyboard (arrows / Enter / Esc / type-ahead)
  // and gamepad (D-pad inside the list, A pick, B close) all work.
  import { tick } from 'svelte'
  import { pushHandler } from '$lib/gamepad/input.svelte'

  interface Option {
    value: T
    label: string
  }
  interface Props {
    value: T
    options: Option[]
    onchange: (value: T) => void
    label?: string
  }
  let { value, options, onchange, label }: Props = $props()

  let open = $state(false)
  let root = $state<HTMLDivElement | undefined>()
  let trigger = $state<HTMLButtonElement | undefined>()
  let list = $state<HTMLDivElement | undefined>()
  let dropUp = $state(false)

  const current = $derived(options.find((o) => o.value === value))

  function items(): HTMLButtonElement[] {
    return Array.from(list?.querySelectorAll<HTMLButtonElement>('button') ?? [])
  }

  async function show() {
    if (open) return
    // open upwards when there is not enough room below
    const r = trigger?.getBoundingClientRect()
    dropUp = !!r && window.innerHeight - r.bottom < Math.min(280, options.length * 40 + 16)
    open = true
    await tick()
    const idx = Math.max(0, options.findIndex((o) => o.value === value))
    items()[idx]?.focus({ preventScroll: true })
    items()[idx]?.scrollIntoView({ block: 'nearest' })
  }

  function close(refocus = true) {
    if (!open) return
    open = false
    if (refocus) trigger?.focus({ preventScroll: true })
  }

  function pick(v: T) {
    if (v !== value) onchange(v)
    close()
  }

  function move(delta: number) {
    const all = items()
    const i = all.indexOf(document.activeElement as HTMLButtonElement)
    const next = all[(i + delta + all.length) % all.length]
    next?.focus({ preventScroll: true })
    next?.scrollIntoView({ block: 'nearest' })
  }

  function onTriggerKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      void show()
    }
  }

  function onListKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      move(1)
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      move(-1)
    } else if (e.key === 'Home') {
      e.preventDefault()
      items()[0]?.focus()
    } else if (e.key === 'End') {
      e.preventDefault()
      items().at(-1)?.focus()
    } else if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault()
      e.stopPropagation()
      close()
    } else if (e.key.length === 1 && /\S/.test(e.key)) {
      // type-ahead: jump to the next option starting with that letter
      const all = items()
      const start = all.indexOf(document.activeElement as HTMLButtonElement)
      const k = e.key.toLowerCase()
      for (let n = 1; n <= all.length; n++) {
        const j = (start + n) % all.length
        if (options[j].label.toLowerCase().startsWith(k)) {
          all[j].focus()
          break
        }
      }
    }
  }

  // Close on outside click.
  $effect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (root && !root.contains(e.target as Node)) close(false)
    }
    window.addEventListener('pointerdown', onDown, true)
    return () => window.removeEventListener('pointerdown', onDown, true)
  })

  // Gamepad: B closes; everything else falls through to spatial navigation,
  // which is confined to the open list by data-focus-scope.
  $effect(() => {
    if (!open) return
    return pushHandler((action) => {
      if (action === 'back') {
        close()
        return true
      }
      return false
    })
  })
</script>

<div class="select" class:open bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="trigger"
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={label}
    onclick={() => (open ? close() : show())}
    onkeydown={onTriggerKey}
  >
    <span class="value">{current?.label ?? value}</span>
    <svg class="chevron" viewBox="0 0 24 24" aria-hidden="true"><path d="M7 10l5 5 5-5" /></svg>
  </button>

  {#if open}
    <div class="list" class:up={dropUp} role="listbox" bind:this={list} data-focus-scope onkeydown={onListKey} tabindex="-1">
      {#each options as o (o.value)}
        <button
          type="button"
          role="option"
          class="option"
          class:selected={o.value === value}
          aria-selected={o.value === value}
          onclick={() => pick(o.value)}
        >
          <span>{o.label}</span>
          {#if o.value === value}
            <svg class="check" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .select {
    position: relative;
    justify-self: end;
    width: 100%;
  }
  .trigger {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--background-darker);
    border: 1px solid var(--background-lighter);
    color: var(--text-default);
    text-align: left;
    transition: border-color var(--transition), background var(--transition);
  }
  .trigger:hover {
    border-color: color-mix(in srgb, var(--accent-overlay) 45%, var(--background-lighter));
  }
  .open .trigger {
    border-color: var(--accent-overlay);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-overlay) 22%, transparent);
  }
  .value {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .chevron {
    flex: none;
    width: 18px;
    height: 18px;
    color: var(--text-secondary);
    transition: transform var(--transition);
  }
  .open .chevron {
    transform: rotate(180deg);
    color: var(--accent-overlay);
  }
  .list {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    right: 0;
    z-index: 60;
    max-height: 280px;
    overflow-y: auto;
    padding: 5px;
    border-radius: 10px;
    background: var(--background);
    border: 1px solid var(--background-lighter);
    box-shadow: 0 14px 40px var(--shadow);
    outline: none;
    animation: drop 140ms ease-out;
    transform-origin: top center;
  }
  .list.up {
    top: auto;
    bottom: calc(100% + 6px);
    transform-origin: bottom center;
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-4px) scale(0.98);
    }
  }
  .option {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 7px;
    color: var(--text-default);
    text-align: left;
    outline: none;
  }
  .option:hover,
  .option:focus {
    background: var(--current-background);
  }
  .option:focus-visible {
    box-shadow: inset 0 0 0 2px var(--accent-overlay);
  }
  .option.selected {
    color: var(--accent-overlay);
    font-weight: 500;
  }
  .check {
    width: 16px;
    height: 16px;
    flex: none;
  }
</style>
