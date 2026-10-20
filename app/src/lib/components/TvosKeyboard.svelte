<script lang="ts">
  // On-screen keyboard overlay. Port of HGL/components/UI/TVOSKeyboard.
  // Focus lives on real <button>s so the ring is visible; the gamepad
  // handler moves it with the same rules as the arrow keys:
  //   Up/Down   preview ↔ key rows ↔ controls (wrapping)
  //   Left/Right caret in the preview, keys within a row, controls
  //   A type · B close · Y space · X backspace · LB/RB switch layout
  import { tick } from 'svelte'
  import {
    LAYOUTS,
    LAYOUT_LABEL,
    QWERTY_ROWS,
    closeKeyboard,
    keyboard,
    moveCaret,
    syncFromTarget,
    typeKey
  } from '$lib/state/keyboard.svelte'
  import { pushHandler, setKeyboardActiveProbe } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { gamepad } from '$lib/gamepad/input.svelte'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'
  import { t } from '$lib/i18n/index.svelte'

  const rows = $derived.by<string[][]>(() =>
    keyboard.mode === 'tvos' ? [LAYOUTS[keyboard.layout]] : QWERTY_ROWS[keyboard.layout]
  )
  const controls = $derived([
    { code: '{layout}', label: LAYOUT_LABEL[keyboard.layout] },
    { code: '{space}', label: t('keyboard.space', 'Space') },
    { code: '{bksp}', label: '⌫' },
    { code: '{clear}', label: t('keyboard.clear', 'Clear') },
    { code: '{done}', label: t('keyboard.done', 'Done') }
  ])

  let previewEl = $state<HTMLDivElement | undefined>()
  let rowsEl = $state<HTMLDivElement | undefined>()
  let controlsEl = $state<HTMLDivElement | undefined>()
  // Buttons are looked up in the DOM on demand; binding a 2-D array of
  // elements is brittle while rows are being re-rendered.
  const keyEls = (r: number): HTMLButtonElement[] =>
    Array.from(rowsEl?.children[r]?.querySelectorAll<HTMLButtonElement>('button') ?? [])
  const controlEls = (): HTMLButtonElement[] =>
    Array.from(controlsEl?.querySelectorAll<HTMLButtonElement>('button') ?? [])
  let focusRow = $state(0)
  let focusCol = $state(0)
  let focusControl = $state(0)

  type Zone = 'preview' | 'keys' | 'controls'
  function zone(): Zone | null {
    const a = document.activeElement
    if (a === previewEl) return 'preview'
    if (a instanceof HTMLElement && a.classList.contains('tvos-key')) return 'keys'
    if (a instanceof HTMLElement && a.classList.contains('tvos-control')) return 'controls'
    return null
  }

  function focusKey(r: number, c: number) {
    const row = keyEls(r)
    if (!row.length) return
    const el = row[Math.min(c, row.length - 1)]
    el?.focus({ preventScroll: true })
    if (keyboard.mode === 'tvos' && el) centerInStrip(el)
  }

  // The tvOS strip is exactly as wide as the search field: the first key
  // starts at the field's left edge, the last one ends at its right edge,
  // and in between the focused key is kept centred. Scrolling only the strip
  // (not scrollIntoView) keeps the rest of the screen still.
  function centerInStrip(el: HTMLElement) {
    const strip = el.parentElement
    if (!strip) return
    const left = el.offsetLeft + el.offsetWidth / 2 - strip.clientWidth / 2
    strip.scrollTo({ left: Math.max(0, Math.min(left, strip.scrollWidth - strip.clientWidth)), behavior: 'smooth' })
  }

  function move(dir: 'up' | 'down' | 'left' | 'right') {
    const z = zone() ?? 'keys'
    if (dir === 'up' || dir === 'down') {
      const down = dir === 'down'
      if (z === 'preview') down ? focusKey(focusRow, focusCol) : controlEls()[focusControl]?.focus()
      else if (z === 'keys') {
        if (down) focusRow < rows.length - 1 ? focusKey(focusRow + 1, focusCol) : controlEls()[focusControl]?.focus()
        else focusRow > 0 ? focusKey(focusRow - 1, focusCol) : previewEl?.focus()
      } else down ? previewEl?.focus() : focusKey(focusRow, focusCol)
      return
    }
    const delta = dir === 'right' ? 1 : -1
    if (z === 'preview') moveCaret(delta)
    else if (z === 'keys') {
      const n = rows[focusRow]?.length ?? 0
      if (n) focusKey(focusRow, (focusCol + delta + n) % n)
    } else {
      const n = controls.length
      controlEls()[(focusControl + delta + n) % n]?.focus()
    }
  }

  function activate() {
    const a = document.activeElement as HTMLElement | null
    if (a?.dataset.key) typeKey(a.dataset.key)
  }

  // Own all gamepad input while open.
  $effect(() => {
    if (!keyboard.open) return
    setKeyboardActiveProbe(() => keyboard.open)
    const off = pushHandler((action) => {
      const d = directionOf(action)
      if (d) {
        move(d)
        return true
      }
      switch (action) {
        case 'mainAction':
          activate()
          return true
        case 'back':
          closeKeyboard()
          return true
        case 'altAction':
          typeKey('{space}')
          return true
        case 'rightClick':
          typeKey('{bksp}')
          return true
        case 'prevPage':
        case 'nextPage':
          typeKey('{layout}')
          return true
        default:
          return true // swallow everything else while typing
      }
    })
    return () => {
      off()
      setKeyboardActiveProbe(() => false)
    }
  })

  // Focus the first key when opened or when the layout/mode changes.
  $effect(() => {
    if (!keyboard.open) return
    void keyboard.layout
    void keyboard.mode
    focusRow = 0
    focusCol = 0
    tick().then(() => focusKey(0, 0))
  })

  // Physical keyboard typing into the target keeps the preview in sync.
  $effect(() => {
    const input = keyboard.target
    if (!input) return
    const h = () => syncFromTarget()
    input.addEventListener('input', h)
    return () => input.removeEventListener('input', h)
  })

  function onKey(e: KeyboardEvent) {
    if (!keyboard.open) return
    const map: Record<string, 'up' | 'down' | 'left' | 'right'> = {
      ArrowUp: 'up',
      ArrowDown: 'down',
      ArrowLeft: 'left',
      ArrowRight: 'right'
    }
    if (map[e.key]) {
      e.preventDefault()
      e.stopPropagation()
      move(map[e.key])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      closeKeyboard()
    } else if (e.key === 'Backspace' && zone() !== null) {
      e.preventDefault()
      typeKey('{bksp}')
    } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && zone() !== null) {
      e.preventDefault()
      typeKey(e.key)
    }
  }

  function keyStyle(i: number): string {
    if (keyboard.mode !== 'tvos') return ''
    const dist = Math.abs(i - focusCol)
    const scale = Math.max(0.6, 1.35 - dist * 0.16)
    const opacity = Math.max(0.3, 1 - dist * 0.2)
    return `transform: scale(${scale}); opacity: ${opacity}`
  }
</script>

<svelte:window onkeydown={onKey} />

{#if keyboard.open}
  <div class="kb-backdrop" data-focus-scope role="presentation" onclick={(e) => e.target === e.currentTarget && closeKeyboard()}>
    <div class="kb">
      <div
        class="preview"
        bind:this={previewEl}
        role="textbox"
        aria-readonly="true"
        aria-label={t('console.search', 'Search')}
        tabindex="0"
      >
        <span class="pt">{keyboard.text.slice(0, keyboard.caret)}</span><span class="caret"></span><span class="pt">{keyboard.text.slice(keyboard.caret)}</span>
        {#if !keyboard.text}<span class="placeholder">{keyboard.placeholder || t('console.searchPlaceholder', 'Search games')}</span>{/if}
      </div>

      <div class="rows" class:carousel={keyboard.mode === 'tvos'} bind:this={rowsEl}>
        {#each rows as row, r (r)}
          <div class="row">
            {#each row as key, c (`${keyboard.layout}-${r}-${key}`)}
              <button
                class="tvos-key"
                data-key={key}
                style={r === focusRow ? keyStyle(c) : ''}
                onfocus={() => {
                  focusRow = r
                  focusCol = c
                }}
                onclick={() => typeKey(key)}
              >{key}</button>
            {/each}
          </div>
        {/each}
      </div>

      <div class="controls" bind:this={controlsEl}>
        {#each controls as ctl, i (ctl.code)}
          <button
            class="tvos-control {ctl.code.slice(1, -1)}"
            data-key={ctl.code}
            onfocus={() => (focusControl = i)}
            onclick={() => typeKey(ctl.code)}
          >{ctl.label}</button>
        {/each}
      </div>

      {#if gamepad.connected}
        <div class="hints">
          <span><ButtonGlyph button="main" layout={gamepad.layout} size={26} /> {t('keyboard.type', 'Type')}</span>
          <span><ButtonGlyph button="y" layout={gamepad.layout} size={26} /> {t('keyboard.space', 'Space')}</span>
          <span><ButtonGlyph button="x" layout={gamepad.layout} size={26} /> ⌫</span>
          <span><ButtonGlyph button="l1" layout={gamepad.layout} size={26} /><ButtonGlyph button="r1" layout={gamepad.layout} size={26} /> {LAYOUT_LABEL[keyboard.layout]}</span>
          <span><ButtonGlyph button="back" layout={gamepad.layout} size={26} /> {t('keyboard.done', 'Done')}</span>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .kb-backdrop {
    position: fixed;
    inset: 0;
    z-index: 900;
    display: flex;
    align-items: center;
    justify-content: center;
    /* light veil: the filtered games stay visible behind the keyboard */
    background: rgba(5, 6, 15, 0.45);
    backdrop-filter: blur(2px);
    user-select: none;
  }
  .kb {
    /* width of the search field; the tvOS strip lines up with it */
    --kb-w: min(1120px, 86vw);
    /* extra room outside the field so the focused key's glow is never cut */
    --kb-bleed: 56px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 28px;
    padding: 16px;
    width: 100%;
  }
  .preview {
    position: relative;
    display: flex;
    align-items: center;
    width: var(--kb-w);
    min-height: 54px;
    padding: 0 24px;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.45);
    border: 1px solid rgba(255, 255, 255, 0.2);
    font-size: 30px;
    font-weight: 600;
    overflow: hidden;
    white-space: nowrap;
    outline: none;
    box-shadow: 0 8px 30px rgba(0, 0, 0, 0.35);
  }
  .preview:focus {
    border-color: var(--accent-overlay);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-overlay) 35%, transparent);
  }
  .pt {
    white-space: pre;
  }
  .placeholder {
    position: absolute;
    left: 28px;
    color: var(--text-tertiary);
    font-weight: 400;
  }
  .caret {
    display: inline-block;
    width: 2px;
    height: 32px;
    margin-left: 2px;
    background: var(--accent-overlay);
    animation: blink 1s steps(2, start) infinite;
  }
  @keyframes blink {
    to {
      opacity: 0;
    }
  }
  .rows {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    max-width: 92vw;
  }
  .row {
    display: flex;
    justify-content: center;
    gap: 12px;
  }
  .carousel {
    width: var(--kb-w);
  }
  .carousel .row {
    position: relative;
    justify-content: flex-start;
    /* The scroll box reaches --kb-bleed past both field edges and is padded
       by the same amount, so the first/last key still lines up with the
       field while its scale + glow have room. Only the far outer edge fades. */
    width: calc(100% + 2 * var(--kb-bleed));
    margin-inline: calc(-1 * var(--kb-bleed));
    gap: 26px;
    padding: 30px var(--kb-bleed) 40px;
    mask-image: linear-gradient(to right, transparent 0, #000 22px, #000 calc(100% - 22px), transparent 100%);
    min-height: 124px;
    overflow-x: auto;
    overflow-y: hidden;
    scroll-behavior: smooth;
    scrollbar-width: none;
  }
  .carousel .row::-webkit-scrollbar {
    display: none;
  }
  .tvos-key,
  .tvos-control {
    flex: 0 0 auto;
    min-width: 44px;
    height: 48px;
    padding: 0 10px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.2);
    background: rgba(0, 0, 0, 0.45);
    color: var(--text-default);
    font-size: 22px;
    font-weight: 600;
    outline: none;
    transform-origin: center bottom;
    transition: transform 0.18s ease, opacity 0.18s ease, background 0.18s ease, box-shadow 0.18s ease;
  }
  .tvos-key:focus,
  .tvos-control:focus {
    background: var(--accent);
    color: var(--accent-text);
    border-color: var(--accent);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent-overlay) 35%, transparent), 0 8px 22px color-mix(in srgb, var(--accent) 45%, transparent);
    z-index: 2;
  }
  .controls {
    display: flex;
    justify-content: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .tvos-control {
    min-width: 56px;
    padding: 0 14px;
    font-size: 18px;
    background: rgba(255, 255, 255, 0.12);
  }
  .tvos-control.space {
    min-width: 160px;
  }
  .tvos-control.clear {
    color: var(--danger);
  }
  .tvos-control.clear:focus {
    background: var(--danger);
    color: #fff;
    border-color: var(--danger);
  }
  .hints {
    display: flex;
    gap: 24px;
    color: var(--text-secondary);
    font-size: 15px;
  }
  .hints span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
</style>
