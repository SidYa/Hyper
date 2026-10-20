<script lang="ts">
  // Frame for console sub-pages (Downloads, Settings): title bar with the
  // clock, scrolling body, controller hints at the bottom.
  import type { Snippet } from 'svelte'
  import { gamepad } from '$lib/gamepad/input.svelte'
  import ControllerHints from '$lib/components/ControllerHints.svelte'
  import type { GlyphButton } from '$lib/components/ButtonGlyph.svelte'
  import ConsoleClock from './ConsoleClock.svelte'

  interface Props {
    title: string
    hints: Array<{ buttons: GlyphButton[]; label: string }>
    children: Snippet
  }
  let { title, hints, children }: Props = $props()
</script>

<div class="page">
  <header>
    <h1>{title}</h1>
    <ConsoleClock />
  </header>
  <div class="body">{@render children()}</div>
  <footer>
    {#if gamepad.connected}<ControllerHints layout={gamepad.layout} {hints} />{/if}
  </footer>
</div>

<style>
  .page {
    height: 100vh;
    display: grid;
    grid-template-rows: auto 1fr auto;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 32px 56px 12px;
  }
  h1 {
    margin: 0;
    font-size: 34px;
    font-weight: 600;
  }
  .body {
    min-height: 0;
    overflow: hidden;
    padding: 12px 56px 0;
  }
  footer {
    display: flex;
    justify-content: center;
    min-height: 56px;
    padding: 10px 56px 22px;
  }
</style>
