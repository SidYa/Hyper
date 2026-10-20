<script lang="ts" module>
  import type { ControllerLayout } from '$lib/gamepad/controller'

  /** Logical buttons, named by role: `main` confirms, `back` cancels, `x` is
   *  the left face button (X / Square), `y` the top one (Y / Triangle). */
  export type GlyphButton = 'main' | 'back' | 'x' | 'y' | 'l1' | 'r1' | 'l2' | 'r2' | 'dpad' | 'stick' | 'start'

  type Face = 'main' | 'back' | 'x' | 'y'
  const SHOULDERS: Record<ControllerLayout, Record<'l1' | 'r1' | 'l2' | 'r2', string>> = {
    xbox: { l1: 'LB', r1: 'RB', l2: 'LT', r2: 'RT' },
    ps4: { l1: 'L1', r1: 'R1', l2: 'L2', r2: 'R2' },
    ps5: { l1: 'L1', r1: 'R1', l2: 'L2', r2: 'R2' },
    'steam-deck': { l1: 'L1', r1: 'R1', l2: 'L2', r2: 'R2' },
    nintendo: { l1: 'L', r1: 'R', l2: 'ZL', r2: 'ZR' }
  }
  const LETTERS: Record<Face, string> = { main: 'A', back: 'B', x: 'X', y: 'Y' }

  // Labels are drawn as strokes instead of <text>: the system font's metrics
  // (a tall ascender, say) would push the letters off-centre. Each character sits in
  // a box `w` wide and 10 high.
  const CHARS: Record<string, { w: number; d: string }> = {
    A: { w: 7, d: 'M0 10L3.5 0L7 10M1.3 6.6H5.7' },
    B: { w: 6.6, d: 'M0 10V0H3.6a2.4 2.4 0 0 1 0 4.8H0M3.6 4.8h0.4a2.6 2.6 0 0 1 0 5.2H0' },
    X: { w: 7, d: 'M0 0L7 10M7 0L0 10' },
    Y: { w: 7, d: 'M0 0L3.5 5L7 0M3.5 5V10' },
    L: { w: 5.4, d: 'M0 0V10H5.4' },
    R: { w: 6.4, d: 'M0 10V0H3.8a2.6 2.6 0 0 1 0 5.2H0M3.2 5.2L6.4 10' },
    T: { w: 7, d: 'M0 0H7M3.5 0V10' },
    Z: { w: 6.4, d: 'M0 0H6.4L0 10H6.4' },
    '1': { w: 3, d: 'M0 2.2L3 0V10' },
    '2': { w: 6, d: 'M0.2 2.6A2.9 2.9 0 1 1 5.2 4.6L0 10H6' }
  }
  const GAP = 1.8

  /** Stroke paths for `text` centred on (cx, cy), `height` units tall. */
  function label(text: string, cx: number, cy: number, height: number, stroke: number) {
    const chars = [...text].map((c) => CHARS[c]).filter(Boolean)
    const width = chars.reduce((sum, c) => sum + c.w, 0) + GAP * (chars.length - 1)
    const scale = height / 10
    let x = 0
    const parts = chars.map((c) => {
      const part = { d: c.d, x }
      x += c.w + GAP
      return part
    })
    return {
      transform: `translate(${cx - (width * scale) / 2} ${cy - height / 2}) scale(${scale})`,
      strokeWidth: stroke / scale,
      parts
    }
  }
</script>

<script lang="ts">
  // Monochrome controller button glyph drawn in currentColor, so it follows
  // the surrounding text colour and any console theme.
  interface Props {
    button: GlyphButton
    layout: ControllerLayout
    size?: number
  }
  let { button, layout, size = 28 }: Props = $props()

  const ps = $derived(layout === 'ps4' || layout === 'ps5')
  const face = $derived(button === 'main' || button === 'back' || button === 'x' || button === 'y')
  const shoulder = $derived(
    button === 'l1' || button === 'r1' || button === 'l2' || button === 'r2' ? SHOULDERS[layout][button] : null
  )
</script>

<svg
  class="glyph"
  width={size}
  height={size}
  viewBox="0 0 32 32"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  role="img"
  aria-label={face ? (ps ? button : LETTERS[button as Face]) : (shoulder ?? button)}
>
  {#if face}
    <circle class="plate" cx="16" cy="16" r="13" />
    {#if ps}
      {#if button === 'main'}
        <path d="M11.5 11.5l9 9m0-9l-9 9" stroke-width="2.4" />
      {:else if button === 'back'}
        <circle cx="16" cy="16" r="5.5" stroke-width="2.2" />
      {:else if button === 'x'}
        <rect x="11" y="11" width="10" height="10" rx="1" stroke-width="2.2" />
      {:else}
        <path d="M16 10l6 10.5H10z" stroke-width="2.2" />
      {/if}
    {:else}
      {@render letters(label(LETTERS[button as Face], 16, 16, 10.5, 2.4))}
    {/if}
  {:else if button === 'l1' || button === 'r1'}
    <rect class="plate" x="2" y="8" width="28" height="16" rx="7" />
    {@render letters(label(shoulder ?? '', 16, 16, 7, 1.7))}
  {:else if button === 'l2' || button === 'r2'}
    <path class="plate" d="M5 27V15a9 9 0 0 1 9-9h4a9 9 0 0 1 9 9v12z" />
    {@render letters(label(shoulder ?? '', 16, 17.5, 7, 1.7))}
  {:else if button === 'start'}
    <!-- Menu / Options / + : a small pill with the family's mark -->
    <rect class="plate" x="5" y="9" width="22" height="14" rx="7" />
    {#if layout === 'nintendo'}
      <path d="M16 12.5v7M12.5 16h7" stroke-width="1.8" />
    {:else}
      <path d="M12 13.5h8M12 16h8M12 18.5h8" stroke-width="1.6" />
    {/if}
  {:else if button === 'dpad'}
    <path class="plate" d="M12.5 3.5h7v9h9v7h-9v9h-7v-9h-9v-7h9z" />
    <path class="fill" d="M16 6.5l2 2.5h-4zM16 25.5l2-2.5h-4zM6.5 16l2.5-2v4zM25.5 16l-2.5-2v4z" stroke="none" />
  {:else}
    <circle class="plate" cx="16" cy="16" r="13" />
    <circle class="fill" cx="16" cy="16" r="6.5" stroke="none" />
  {/if}
</svg>

{#snippet letters(l: ReturnType<typeof label>)}
  <g transform={l.transform} stroke-width={l.strokeWidth}>
    {#each l.parts as p, i (i)}<path d={p.d} transform="translate({p.x} 0)" />{/each}
  </g>
{/snippet}

<style>
  .glyph {
    display: inline-block;
    flex: none;
    vertical-align: middle;
  }
  .plate {
    fill: currentColor;
    fill-opacity: 0.14;
  }
  .fill {
    fill: currentColor;
    fill-opacity: 0.55;
  }
</style>
