<script lang="ts">
  // The download queue: the running item first, then the ones waiting. Each
  // row has the cover, a progress bar like the Library tiles, and the
  // numbers (done / total, network and disk speed, time left).
  import { cancel, findGame, library, resume, statusKey } from '$lib/state/library.svelte'
  import { navigate } from '$lib/router.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { formatBytes, formatEta, formatSpeed } from '$lib/format'
  import { cardImage, runnerName } from '$lib/library/view'
  import type { DownloadItem } from '$types/index'

  const ACTIVE = ['installing', 'updating', 'repairing']
  let broken = $state<Record<string, true>>({})

  const key = (item: DownloadItem) => `${item.runner}:${item.appName}`
  // running per the queue, or per the game's own status (whichever is newer)
  const isRunning = (item: DownloadItem) =>
    item.running || ACTIVE.includes(library.statuses[statusKey(item.runner, item.appName)]?.status ?? '')

  function kindLabel(item: DownloadItem) {
    if (item.kind === 'update') return t('game.update', 'Update')
    if (item.kind === 'repair') return t('downloads.repair', 'Verify files')
    return t('console.install', 'Install')
  }
  function stateLabel(item: DownloadItem) {
    if (item.kind === 'update') return t('game.updating', 'Updating')
    if (item.kind === 'repair') return t('downloads.verifying', 'Verifying')
    return t('game.installing', 'Installing')
  }

  const queued = $derived(library.downloads.filter((d) => !isRunning(d) && !d.paused))
</script>

<div class="downloads">
  <header>
    <h1>{t('downloads.title', 'Downloads')}</h1>
    {#if library.downloads.length}
      <span class="muted">{t('nav.downloadsActive', '{{count}} in queue', { count: library.downloads.length })}</span>
    {/if}
  </header>

  {#if library.downloads.length === 0}
    <div class="empty">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4v11M7 10.5l5 5 5-5M5 20h14" /></svg>
      <p>{t('downloads.empty', 'Nothing is downloading')}</p>
    </div>
  {:else}
    <ul>
      {#each library.downloads as item (key(item))}
        {@const s = library.statuses[statusKey(item.runner, item.appName)]}
        {@const game = findGame(item.runner, item.appName)}
        {@const img = game ? cardImage(game) : ''}
        {@const running = isRunning(item)}
        {@const p = running && s?.progress && s.progress.bytesTotal > 0 ? s.progress : undefined}
        {@const note = running ? s?.message : undefined}
        <li class:running class:waiting={!running} class:paused={item.paused}>
          <button class="cover" onclick={() => navigate(`/game/${item.runner}/${item.appName}`)} aria-label={item.title}>
            {#if img && !broken[key(item)]}
              <img src={img} alt="" loading="lazy" onerror={() => (broken[key(item)] = true)} />
            {:else}
              <span class="initial">{item.title[0]}</span>
            {/if}
          </button>

          <div class="body">
            <div class="head">
              <div class="names">
                <button class="title" onclick={() => navigate(`/game/${item.runner}/${item.appName}`)}>{item.title}</button>
                <span class="sub">{runnerName(item.runner)} · {kindLabel(item)}</span>
              </div>
              {#if p}
                <span class="pct">{Math.floor(p.percent)}<small>%</small></span>
              {/if}
            </div>

            <div class="track" class:indeterminate={running && !p}>
              <div class="fill" style:width={`${p ? Math.max(0, Math.min(100, p.percent)) : 0}%`}></div>
            </div>

            <div class="stats">
              {#if p}
                <span class="state">{note ?? stateLabel(item)}</span>
                <span>{formatBytes(p.bytesDone)} / {formatBytes(p.bytesTotal)}</span>
                <span class="metric" title={t('downloads.network', 'Network')}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 4v12M7 11l5 5 5-5M5 20h14" /></svg>
                  {formatSpeed(p.speed)}
                </span>
                <span class="metric" title={t('downloads.disk', 'Disk')}>
                  <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="6" width="18" height="12" rx="2.5" /><path d="M7 14h.01M11 14h6" /></svg>
                  {formatSpeed(p.diskSpeed)}
                </span>
                {#if p.eta != null}<span class="eta">{t('downloads.eta', '{{time}} left', { time: formatEta(p.eta) })}</span>{/if}
              {:else if running}
                <span class="state">{note ?? t('downloads.preparing', 'Preparing…')}</span>
              {:else if item.paused}
                <span class="state">{t('downloads.paused', 'Paused')}</span>
                <span>{t('downloads.pausedHint', 'Left from the last session; continues where it stopped')}</span>
              {:else}
                <span>{t('downloads.queued', 'Queued')} · {t('downloads.position', '#{{n}} in line', { n: queued.indexOf(item) + 1 })}</span>
              {/if}
            </div>
          </div>

          {#if item.paused}
            <button class="resume" onclick={() => resume(item.runner, item.appName)} title={t('downloads.resume', 'Resume')} aria-label={t('downloads.resume', 'Resume')}>
              <svg viewBox="0 0 24 24" aria-hidden="true"><path class="solid" d="M8 5.5v13l10.5-6.5z" /></svg>
            </button>
          {/if}
          <button class="cancel" onclick={() => cancel(item.runner, item.appName)} title={item.paused ? t('downloads.remove', 'Remove') : t('game.cancel', 'Cancel')} aria-label={item.paused ? t('downloads.remove', 'Remove') : t('game.cancel', 'Cancel')}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .downloads {
    padding: var(--space-lg) var(--space-xl);
  }
  header {
    display: flex;
    align-items: baseline;
    gap: var(--space-md);
    margin-bottom: var(--space-lg);
  }
  h1 {
    margin: 0;
    font-size: 26px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: var(--space-xl) 0;
    color: var(--text-secondary);
  }
  .empty svg {
    width: 44px;
    height: 44px;
    fill: none;
    stroke: var(--text-tertiary);
    stroke-width: 1.6;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  ul {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--space-md);
    padding: 12px;
    border-radius: 12px;
    background: var(--background);
    border: 1px solid var(--background-lighter);
    transition: opacity 150ms ease;
  }
  li.running {
    border-color: color-mix(in srgb, var(--accent) 45%, var(--background-lighter));
    box-shadow: 0 6px 22px color-mix(in srgb, var(--accent) 12%, transparent);
  }
  li.waiting {
    opacity: 0.75;
  }
  .cover {
    flex: none;
    width: 64px;
    aspect-ratio: 3 / 4;
    border-radius: 8px;
    overflow: hidden;
    padding: 0;
    display: grid;
    place-items: center;
    background: linear-gradient(160deg, var(--background-lighter), var(--background-darker));
  }
  .cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  li.waiting .cover img {
    filter: grayscale(0.7);
  }
  .initial {
    font-size: 24px;
    font-weight: 700;
    color: var(--text-tertiary);
  }
  .body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px;
  }
  .names {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .title {
    padding: 0;
    text-align: left;
    font-size: 15px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title:hover {
    color: var(--accent-overlay);
  }
  .sub {
    font-size: 12px;
    color: var(--text-tertiary);
  }
  .pct {
    flex: none;
    font-size: 24px;
    font-weight: 700;
    line-height: 1;
    font-variant-numeric: tabular-nums;
    color: var(--accent-overlay);
  }
  .pct small {
    font-size: 14px;
    margin-left: 1px;
  }
  .track {
    position: relative;
    height: 8px;
    border-radius: 999px;
    overflow: hidden;
    background: var(--background-darker);
  }
  .fill {
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, color-mix(in srgb, var(--accent-overlay) 70%, transparent), var(--accent-overlay));
    transition: width 300ms linear;
  }
  /* preparing: a light sweeps across the empty track */
  .track.indeterminate::after {
    content: '';
    position: absolute;
    inset: 0;
    width: 35%;
    border-radius: inherit;
    background: linear-gradient(90deg, transparent, color-mix(in srgb, var(--accent-overlay) 60%, transparent), transparent);
    animation: sweep 1.3s ease-in-out infinite;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(300%);
    }
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    font-size: 12px;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }
  .state {
    color: var(--text-default);
    font-weight: 500;
  }
  .metric {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .metric svg {
    width: 14px;
    height: 14px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.8;
    stroke-linecap: round;
    stroke-linejoin: round;
    opacity: 0.8;
  }
  .eta {
    margin-left: auto;
  }
  li.paused {
    opacity: 1;
    border-style: dashed;
  }
  .cancel,
  .resume {
    flex: none;
    width: 34px;
    height: 34px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    color: var(--text-tertiary);
  }
  .resume {
    color: var(--accent-overlay);
  }
  .resume:hover {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
  }
  .resume svg {
    width: 18px;
    height: 18px;
  }
  .resume .solid {
    fill: currentColor;
    stroke: none;
  }
  .cancel:hover {
    background: color-mix(in srgb, var(--danger) 15%, transparent);
    color: var(--danger);
  }
  .cancel svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
  }
</style>
