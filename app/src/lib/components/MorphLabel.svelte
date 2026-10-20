<script lang="ts">
  // A button label that morphs when its text changes ("Play" → "Preparing"):
  // the old text slides up and out while the new one slides in, both in the
  // same grid cell so the button does not jump. `busy` adds a light sweep over
  // the text and three pulsing dots while the work goes on.
  import { fly } from 'svelte/transition'
  import { cubicOut } from 'svelte/easing'

  interface Props {
    text: string
    busy?: boolean
  }
  let { text, busy = false }: Props = $props()

  const still = typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
  const enter = { y: 12, duration: still ? 0 : 280, easing: cubicOut }
  const leave = { y: -12, duration: still ? 0 : 200, easing: cubicOut }
</script>

<span class="morph">
  {#key text}
    <span class="txt" class:busy in:fly={enter} out:fly={leave}>
      <span class="word" data-text={text}>{text}</span>
      {#if busy}<span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>{/if}
    </span>
  {/key}
</span>

<style>
  .morph {
    display: inline-grid;
    overflow: hidden;
    /* room for the slide without clipping descenders */
    padding: 2px 0;
  }
  .txt {
    grid-area: 1 / 1;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  .word {
    position: relative;
  }
  /* a band of light sweeping over the letters, drawn on top of them */
  .busy .word::after {
    content: attr(data-text);
    position: absolute;
    inset: 0;
    background: linear-gradient(100deg, transparent 35%, rgba(255, 255, 255, 0.95) 50%, transparent 65%);
    background-size: 250% 100%;
    background-clip: text;
    -webkit-background-clip: text;
    color: transparent;
    animation: sweep 1.8s linear infinite;
  }
  .dots {
    display: inline-flex;
    gap: 3px;
    align-items: flex-end;
    height: 0.9em;
  }
  .dots i {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: currentColor;
    animation: pulse 1.2s ease-in-out infinite;
  }
  .dots i:nth-child(2) {
    animation-delay: 0.15s;
  }
  .dots i:nth-child(3) {
    animation-delay: 0.3s;
  }
  @keyframes sweep {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -150% 0;
    }
  }
  @keyframes pulse {
    0%,
    70%,
    100% {
      opacity: 0.35;
      transform: translateY(0);
    }
    35% {
      opacity: 1;
      transform: translateY(-4px);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .busy .word::after {
      display: none;
    }
    .dots i {
      animation: none;
    }
  }
</style>
