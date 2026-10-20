<script lang="ts">
  // Console tile. While installing/updating, the cover "fills with colour"
  // from the top over a black-and-white copy, with a thin percentage at the
  // bottom centre. Games added by hand without art get a generated cover.
  import { cardImage, imageSrc, runnerName } from '$lib/library/view'
  import AutoCover from '$lib/components/AutoCover.svelte'
  import type { GameInfo, GameStatus } from '$types/index'

  interface Props {
    game: GameInfo
    status?: GameStatus
    focused: boolean
    showTitle?: boolean
    /** the store's name under the title */
    showStore?: boolean
    showPercent?: boolean
    onfocus: () => void
    onselect: () => void
  }
  let {
    game,
    status,
    focused,
    showTitle = false,
    showStore = false,
    showPercent = true,
    onfocus,
    onselect
  }: Props = $props()

  const img = $derived(cardImage(game))
  let broken = $state(false)

  const downloading = $derived(status?.status === 'installing' || status?.status === 'updating')
  const percent = $derived(downloading ? Math.max(0, Math.min(100, status?.progress?.percent ?? 0)) : 0)

  type Icon = 'check' | 'update' | 'queued' | 'paused' | 'play' | 'busy' | null
  const icon = $derived.by<Icon>(() => {
    switch (status?.status) {
      case 'queued':
        return 'queued'
      case 'paused':
        return 'paused'
      case 'playing':
        return 'play'
      case 'preparing':
      case 'launching':
      case 'syncing':
      case 'uninstalling':
      case 'moving':
        return 'busy'
      case 'installing':
      case 'updating':
        return null
    }
    if (game.updateAvailable) return 'update'
    return game.isInstalled ? 'check' : null
  })
</script>

<button
  data-item
  class="card"
  class:focused
  class:installed={game.isInstalled}
  class:downloading
  onmouseenter={onfocus}
  onclick={onselect}
  tabindex="-1"
>
  <div class="art">
    {#if img && !broken}
      <img class="cover" src={img} alt="" loading="lazy" decoding="async" onerror={() => (broken = true)} />
      {#if downloading}
        <img
          class="cover colour"
          src={img}
          alt=""
          style:--p={`${percent}%`}
        />
      {/if}
    {:else if game.runner === 'sideload'}
      <AutoCover title={game.title} icon={imageSrc(game.artIcon)} />
    {:else}
      <span class="initial">{game.title[0]}</span>
    {/if}

    {#if downloading && showPercent}
      <span class="percent">{Math.floor(percent)}%</span>
    {/if}

    {#if icon}
      <span class="status" class:ok={icon === 'check'}>
        {#if icon === 'check'}
          <!-- installed: just the green indicator -->
        {:else if icon === 'update'}
          <svg viewBox="0 0 24 24"><path d="M12 19V5M6 11l6-6 6 6" /></svg>
        {:else if icon === 'queued'}
          <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8" /><path d="M12 8v4.5l3 2" /></svg>
        {:else if icon === 'paused'}
          <svg viewBox="0 0 24 24"><path d="M9 6v12M15 6v12" /></svg>
        {:else if icon === 'play'}
          <svg viewBox="0 0 24 24"><path class="solid" d="M8 5.5v13l10.5-6.5z" /></svg>
        {:else}
          <svg viewBox="0 0 24 24" class="spin"><path d="M12 4a8 8 0 1 1-8 8" /></svg>
        {/if}
      </span>
    {/if}
  </div>
  {#if showTitle}
    <div class="meta">
      <span class="title">{game.title}</span>
      {#if showStore}<span class="store">{runnerName(game.runner)}</span>{/if}
    </div>
  {/if}
</button>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 0;
    text-align: left;
    border-radius: 14px;
    outline: 3px solid transparent;
    outline-offset: 4px;
    transition: transform 140ms ease, outline-color 140ms ease;
  }
  .card.focused {
    transform: scale(1.06);
    outline-color: var(--console-card-focus, var(--console-focus, #ffffff));
    z-index: 1;
  }
  /* a soft glow of the same colour around the cover */
  .card.focused .art {
    box-shadow:
      0 10px 30px rgba(0, 0, 0, 0.45),
      0 0 28px color-mix(in srgb, var(--console-card-focus, transparent) 45%, transparent);
  }
  .art {
    position: relative;
    aspect-ratio: 3 / 4;
    border-radius: 14px;
    overflow: hidden;
    background: linear-gradient(160deg, var(--background-lighter), var(--background-darker));
    display: grid;
    place-items: center;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45);
  }
  .cover {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .card:not(.installed) .cover:not(.colour) {
    filter: saturate(0.6) brightness(0.8);
  }
  .card.downloading .cover:not(.colour) {
    filter: grayscale(1) brightness(0.55);
  }
  /* colour copy revealed with a soft edge that follows --p */
  .colour {
    /* install progress fills the colour in from the top */
    -webkit-mask-image: linear-gradient(to bottom, #000 calc(var(--p) - 6%), transparent calc(var(--p) + 6%));
    mask-image: linear-gradient(to bottom, #000 calc(var(--p) - 6%), transparent calc(var(--p) + 6%));
    transition: --p 900ms linear;
  }
  .percent {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 14px;
    text-align: center;
    font-size: 30px;
    font-weight: 200;
    letter-spacing: 1px;
    color: #fff;
    text-shadow: 0 2px 12px rgba(0, 0, 0, 0.8);
  }
  .initial {
    position: relative;
    font-size: 64px;
    font-weight: 700;
    color: var(--text-tertiary);
  }
  .status {
    position: absolute;
    top: 10px;
    right: 10px;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(10, 12, 24, 0.6);
    backdrop-filter: blur(6px);
    border: 1px solid rgba(255, 255, 255, 0.18);
  }
  /* installed: a small glowing green dot instead of a badge */
  .status.ok {
    /* bottom-right corner, away from the other badges */
    top: auto;
    bottom: 14px;
    right: 14px;
    width: 14px;
    height: 14px;
    background: var(--success);
    border: none;
    backdrop-filter: none;
    box-shadow:
      0 0 0 3px rgba(10, 12, 24, 0.55),
      0 0 10px color-mix(in srgb, var(--success) 70%, transparent);
  }
  .status svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: #fff;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .status svg .solid {
    fill: #fff;
    stroke: none;
  }
  .spin {
    animation: spin 0.9s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .meta {
    display: flex;
    flex-direction: column;
    padding: 0 4px;
  }
  .title {
    font-size: 18px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .store {
    font-size: 13px;
    color: var(--text-secondary);
  }
</style>
