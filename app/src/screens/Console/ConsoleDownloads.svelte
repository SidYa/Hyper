<script lang="ts">
  // Console-mode download queue: cover, progress and speed per game; A asks
  // to cancel the focused download (to resume a paused one), Y removes a
  // paused one, B goes back to the library.
  import { tick } from 'svelte'
  import { cancel, library, resume, statusKey } from '$lib/state/library.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { formatBytes, formatEta, formatSpeed } from '$lib/format'
  import { cardImage } from '$lib/library/view'
  import { itemAt } from '$lib/dom'
  import { RUNNER_NAMES, type DownloadItem } from '$types/index'
  import ConsolePage from './ConsolePage.svelte'
  import ConsoleDialog from './ConsoleDialog.svelte'

  interface Props {
    onback: () => void
  }
  let { onback }: Props = $props()

  let index = $state(0)
  let confirm = $state<DownloadItem | null>(null)
  let resumeAsk = $state<DownloadItem | null>(null)
  let listEl = $state<HTMLElement | undefined>()

  const items = $derived(library.downloads)
  $effect(() => {
    if (index >= items.length) index = Math.max(0, items.length - 1)
  })
  $effect(() => {
    void index
    tick().then(() => itemAt(listEl, index)?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }))
  })

  const gameOf = (d: DownloadItem) => library.games.find((g) => g.runner === d.runner && g.appName === d.appName)

  function doCancel() {
    const d = confirm
    confirm = null
    if (d) void cancel(d.runner, d.appName)
  }
  function doResume() {
    const d = resumeAsk
    resumeAsk = null
    if (d) void resume(d.runner, d.appName)
  }
  // A on a paused item asks to resume it, on the others to cancel
  function activate(item: DownloadItem) {
    if (item.paused) resumeAsk = item
    else confirm = item
  }

  $effect(() =>
    pushHandler((action) => {
      const dir = directionOf(action)
      if (confirm) {
        if (action === 'mainAction') doCancel()
        else if (action === 'back') confirm = null
        return true
      }
      if (resumeAsk) {
        if (action === 'mainAction') doResume()
        else if (action === 'back') resumeAsk = null
        return true
      }
      if (dir === 'up') index = Math.max(0, index - 1)
      else if (dir === 'down') index = Math.min(items.length - 1, index + 1)
      else if (action === 'mainAction' && items[index]) activate(items[index])
      else if (action === 'altAction' && items[index]?.paused) confirm = items[index]
      else if (action === 'back') onback()
      return true
    })
  )

  const focusedPaused = $derived(!!items[index]?.paused)
  const hints = $derived([
    ...(items.length ? [{ buttons: ['main' as const], label: focusedPaused ? t('downloads.resume', 'Resume') : t('game.cancel', 'Cancel') }] : []),
    ...(focusedPaused ? [{ buttons: ['y' as const], label: t('downloads.remove', 'Remove') }] : []),
    { buttons: ['back' as const], label: t('console.back', 'Back') }
  ])
</script>

<ConsolePage title={t('downloads.title', 'Downloads')} {hints}>
  {#if items.length === 0}
    <div class="empty">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v11m-4.5-4.5L12 14l4.5-4.5M4 16v3a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-3" /></svg>
      {t('downloads.empty', 'Nothing is downloading')}
    </div>
  {:else}
    <ul bind:this={listEl}>
      {#each items as item, i (`${item.runner}:${item.appName}`)}
        {@const s = library.statuses[statusKey(item.runner, item.appName)]}
        {@const game = gameOf(item)}
        <li>
          <button
            class="item"
            class:focused={i === index}
            data-item
            onmouseenter={() => (index = i)}
            onclick={() => activate(item)}
          >
            {#if game && cardImage(game)}<img src={cardImage(game)} alt="" />{:else}<span class="ph"></span>{/if}
            <span class="info">
              <span class="title">{item.title}</span>
              <span class="meta">
                {RUNNER_NAMES[item.runner]} · {item.kind === 'update' ? t('game.update', 'Update') : item.kind === 'repair' ? t('downloads.repair', 'Verify files') : t('console.install', 'Install')}
              </span>
              {#if item.running && s?.progress && s.progress.bytesTotal > 0}
                <span class="bar"><span class="fill" style:width={`${s.progress.percent}%`}></span></span>
                <span class="meta">
                  {s.progress.percent.toFixed(1)}% · {formatBytes(s.progress.bytesDone)} / {formatBytes(s.progress.bytesTotal)}
                  · {formatSpeed(s.progress.speed)} · {formatEta(s.progress.eta)}
                </span>
              {:else if item.running}
                <span class="meta">{s?.message ?? t('downloads.preparing', 'Preparing…')}</span>
              {:else if item.paused}
                <span class="meta">{t('downloads.paused', 'Paused')} · {t('downloads.pausedHint', 'Left from the last session; continues where it stopped')}</span>
              {:else}
                <span class="meta">{t('downloads.queued', 'Queued')}</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</ConsolePage>

{#if confirm}
  <ConsoleDialog
    title={confirm.paused ? t('console.removeFromQueue.title', 'Remove from queue?') : t('console.cancelDownloadQ', 'Cancel download?')}
    gameTitle={confirm.title}
    confirmLabel={confirm.paused ? t('console.remove', 'Remove') : t('console.cancelDownload', 'Cancel download')}
    cancelLabel={t('console.keep', 'Keep')}
    danger
    onconfirm={doCancel}
    oncancel={() => (confirm = null)}
  />
{:else if resumeAsk}
  <ConsoleDialog
    title={t('console.resumeDownloadQ.title', 'Resume download?')}
    message={t('console.resumeDownloadQ.message', 'This download was left from the last session and continues where it stopped.')}
    gameTitle={resumeAsk.title}
    confirmLabel={t('downloads.resume', 'Resume')}
    cancelLabel={t('console.keep', 'Keep')}
    onconfirm={doResume}
    oncancel={() => (resumeAsk = null)}
  />
{/if}

<style>
  ul {
    list-style: none;
    margin: 0;
    /* room for the 3px focus ring: a scroll container clips box-shadow */
    padding: 6px 6px 40px;
    height: 100%;
    overflow-y: auto;
    scrollbar-width: none;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 24px;
    padding: 14px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.05);
    text-align: left;
    color: var(--text-default);
    outline: none;
    transition: box-shadow 120ms ease, background 120ms ease;
  }
  .item.focused {
    background: rgba(255, 255, 255, 0.1);
    box-shadow: 0 0 0 3px var(--console-focus);
  }
  img,
  .ph {
    width: 90px;
    height: 120px;
    flex: none;
    border-radius: 8px;
    object-fit: cover;
    background: rgba(255, 255, 255, 0.08);
  }
  .info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .title {
    font-size: 22px;
    font-weight: 600;
  }
  .meta {
    font-size: 16px;
    color: var(--text-secondary);
  }
  .bar {
    height: 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 400ms ease;
  }
  .empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 18px;
    font-size: 22px;
    color: var(--text-secondary);
  }
  .empty svg {
    width: 64px;
    height: 64px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-linejoin: round;
    opacity: 0.6;
  }
</style>
