<script lang="ts">
  // Screenshot strip of the desktop game page: thumbnails that scroll
  // sideways (arrows at the edges when they overflow) and open a full-screen
  // viewer with previous / next (buttons, ← →, Esc or a click outside closes).
  // The desktop twin of the strip in ConsoleGame.
  import { screenshotUrl } from '$lib/library/view'
  import { t } from '$lib/i18n/index.svelte'

  interface Props {
    shots: string[]
    /** store details still loading: placeholders instead of nothing */
    loading?: boolean
  }
  let { shots, loading = false }: Props = $props()

  let strip = $state<HTMLElement>()
  let canLeft = $state(false)
  let canRight = $state(false)
  let viewer = $state<number | null>(null)

  function updateArrows() {
    if (!strip) return
    canLeft = strip.scrollLeft > 4
    canRight = strip.scrollLeft + strip.clientWidth < strip.scrollWidth - 4
  }
  $effect(() => {
    if (!strip) return
    void shots.length
    updateArrows()
    const ro = new ResizeObserver(updateArrows)
    ro.observe(strip)
    return () => ro.disconnect()
  })

  function page(dir: -1 | 1) {
    strip?.scrollBy({ left: dir * strip.clientWidth * 0.8, behavior: 'smooth' })
  }

  function step(dir: -1 | 1) {
    if (viewer === null) return
    viewer = (viewer + dir + shots.length) % shots.length
  }

  $effect(() => {
    if (viewer === null) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') viewer = null
      else if (e.key === 'ArrowLeft') step(-1)
      else if (e.key === 'ArrowRight') step(1)
      else return
      e.preventDefault()
      e.stopPropagation()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  })
</script>

{#if shots.length || loading}
  <section class="gallery">
    <h2>{t('console.game.screenshots', 'Screenshots')}</h2>
    <div class="frame">
      <div class="strip" bind:this={strip} onscroll={updateArrows}>
        {#if shots.length}
          {#each shots as src, i (src)}
            <button class="thumb" onclick={() => (viewer = i)} aria-label={`${t('console.game.screenshots', 'Screenshots')} ${i + 1}`}>
              <img src={screenshotUrl(src, 640)} alt="" loading="lazy" decoding="async" />
            </button>
          {/each}
        {:else}
          <span class="thumb ph"></span><span class="thumb ph"></span><span class="thumb ph"></span>
        {/if}
      </div>
      {#if canLeft}
        <button class="nav left" onclick={() => page(-1)} aria-label={t('gallery.previous', 'Previous')}>
          <svg viewBox="0 0 24 24"><path d="M15 5l-7 7 7 7" /></svg>
        </button>
      {/if}
      {#if canRight}
        <button class="nav right" onclick={() => page(1)} aria-label={t('gallery.next', 'Next')}>
          <svg viewBox="0 0 24 24"><path d="M9 5l7 7-7 7" /></svg>
        </button>
      {/if}
    </div>
  </section>
{/if}

{#if viewer !== null}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="viewer" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.target === e.currentTarget && (viewer = null)}>
    {#key viewer}
      <img src={screenshotUrl(shots[viewer], 1920)} alt="" />
    {/key}
    {#if shots.length > 1}
      <button class="vnav left" onclick={() => step(-1)} aria-label={t('gallery.previous', 'Previous')}>
        <svg viewBox="0 0 24 24"><path d="M15 5l-7 7 7 7" /></svg>
      </button>
      <button class="vnav right" onclick={() => step(1)} aria-label={t('gallery.next', 'Next')}>
        <svg viewBox="0 0 24 24"><path d="M9 5l7 7-7 7" /></svg>
      </button>
    {/if}
    <span class="count">{viewer + 1} / {shots.length}</span>
    <button class="close" onclick={() => (viewer = null)} aria-label={t('gallery.close', 'Close')}>
      <svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18" /></svg>
    </button>
  </div>
{/if}

<style>
  .gallery {
    margin: var(--space-lg) var(--space-xl) 0;
  }
  h2 {
    margin: 0 0 10px;
    font-size: 13px;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--text-secondary);
  }
  .frame {
    position: relative;
  }
  .strip {
    display: flex;
    gap: 12px;
    overflow-x: auto;
    scroll-snap-type: x mandatory;
    scrollbar-width: none;
    /* the strip clips what overflows it: room around the thumbnails for
       their hover lift, ring and shadow (the negative margin keeps them
       aligned with the page) */
    padding: 12px 20px 28px;
    margin: -12px -20px -28px;
    scroll-padding-inline: 20px;
  }
  .strip::-webkit-scrollbar {
    display: none;
  }
  .thumb {
    flex: 0 0 auto;
    height: 150px;
    aspect-ratio: 16 / 9;
    padding: 0;
    border-radius: 10px;
    overflow: hidden;
    scroll-snap-align: start;
    background: var(--background);
    box-shadow: 0 0 0 1px var(--background-lighter);
    transition:
      transform var(--transition),
      box-shadow var(--transition);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  button.thumb:hover,
  button.thumb:focus-visible {
    transform: translateY(-3px);
    box-shadow:
      0 0 0 2px var(--accent-overlay),
      0 10px 24px rgba(0, 0, 0, 0.4);
    outline: none;
  }
  .ph {
    animation: pulse 1.1s ease-in-out infinite alternate;
  }
  .nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 40px;
    height: 40px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    background: rgba(10, 10, 20, 0.7);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.45);
    backdrop-filter: blur(6px);
    transition: background var(--transition);
  }
  .nav:hover {
    background: var(--accent-overlay);
  }
  .nav.left {
    left: -12px;
  }
  .nav.right {
    right: -12px;
  }
  .nav svg,
  .vnav svg,
  .close svg {
    width: 22px;
    height: 22px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .viewer {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.9);
    animation: fade 160ms ease-out;
  }
  .viewer img {
    max-width: 88vw;
    max-height: 84vh;
    border-radius: 12px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.6);
    animation: zoom 200ms ease-out;
  }
  .vnav,
  .close {
    position: absolute;
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 50%;
    color: #fff;
    background: rgba(255, 255, 255, 0.08);
    transition: background var(--transition);
  }
  .vnav:hover,
  .close:hover {
    background: var(--accent-overlay);
  }
  .vnav {
    top: 50%;
    transform: translateY(-50%);
  }
  .vnav.left {
    left: 24px;
  }
  .vnav.right {
    right: 24px;
  }
  /* below the window controls, which stay on top */
  .close {
    top: 52px;
    right: 24px;
    width: 44px;
    height: 44px;
  }
  .count {
    position: absolute;
    bottom: 24px;
    font-size: 15px;
    color: var(--text-secondary);
  }
  @keyframes pulse {
    to {
      opacity: 0.5;
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes zoom {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .viewer,
    .viewer img,
    .ph {
      animation: none;
    }
  }
</style>
