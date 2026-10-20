<script lang="ts">
  // Footer with button hints drawn as monochrome glyphs for the connected
  // controller family.
  import type { ControllerLayout } from '$lib/gamepad/controller'
  import ButtonGlyph, { type GlyphButton } from './ButtonGlyph.svelte'

  interface Hint {
    buttons: GlyphButton[]
    label: string
  }
  interface Props {
    layout: ControllerLayout
    hints: Hint[]
  }
  let { layout, hints }: Props = $props()
</script>

<div class="hints">
  {#each hints as h (h.label)}
    <span class="hint">
      {#each h.buttons as b (b)}<ButtonGlyph button={b} {layout} />{/each}
      {h.label}
    </span>
  {/each}
</div>

<style>
  .hints {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    align-items: center;
    gap: 6px 22px;
    color: var(--text-secondary);
    font-size: 15px;
  }
  .hint {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
</style>
