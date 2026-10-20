<script lang="ts">
  // Cloud saves button next to Play: a drop-down with the manual sync
  // actions and, below a separator, the save folder.
  import { errorMessage, openSaveFolder, syncSaves } from '$lib/api/commands'
  import { gameStatus, savesPending } from '$lib/state/library.svelte'
  import { notify } from '$lib/state/toast.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import type { CloudSaves, GameInfo, SaveSyncMode } from '$types/index'

  interface Props {
    game: GameInfo
    saves: CloudSaves
    /** Called after a sync (folders may exist now). */
    onsynced: () => void
  }
  let { game, saves, onsynced }: Props = $props()

  let open = $state(false)
  let syncing = $state(false)
  let root = $state<HTMLElement>()

  const status = $derived(gameStatus(game))
  const autoSyncing = $derived(status?.status === 'syncing')
  // the game runs or installs, or its saves already sync
  const blocked = $derived(!!status || syncing)
  const ready = $derived(saves.locations.length > 0 && saves.locations.every((l) => l.path))
  const folder = $derived(saves.locations.find((l) => l.exists))
  // played offline: the cloud does not have these saves yet
  const pending = $derived(savesPending(game, saves.pendingUpload))

  // the newer side wins either way; no forced overwrites (owner decision)
  const items: Array<{ mode: SaveSyncMode; label: () => string; hint: () => string }> = [
    { mode: 'download', label: () => t('saves.download', 'Download'), hint: () => t('saves.downloadHint', 'Get newer saves from the cloud') },
    { mode: 'upload', label: () => t('saves.upload', 'Upload'), hint: () => t('saves.uploadHint', 'Send newer saves from this PC to the cloud') }
  ]

  // close on a click elsewhere or Escape
  $effect(() => {
    if (!open) return
    const onDown = (e: PointerEvent) => {
      if (root && !root.contains(e.target as Node)) open = false
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') open = false
    }
    window.addEventListener('pointerdown', onDown, true)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('pointerdown', onDown, true)
      window.removeEventListener('keydown', onKey)
    }
  })

  async function sync(mode: SaveSyncMode) {
    open = false
    if (!game.runner) return
    syncing = true
    try {
      const summary = await syncSaves(game.runner, game.appName, mode)
      const lastLine = summary.split('\n').filter(Boolean).pop()
      notify(
        t('saves.synced', 'Saves of {{title}} synced', { title: game.title }) + (lastLine ? `: ${lastLine}` : ''),
        'success',
        6000
      )
    } catch (e) {
      notify(errorMessage(e), 'error', 9000)
    } finally {
      syncing = false
      onsynced()
    }
  }

  function openFolder() {
    open = false
    if (!game.runner || !folder) return
    openSaveFolder(game.runner, game.appName, folder.name).catch((e) => notify(errorMessage(e), 'error'))
  }
</script>

<div class="wrap" bind:this={root}>
  <button
    class="icon"
    class:active={open}
    title={!ready
      ? t('saves.notFound', 'The save folder of this game could not be found.')
      : syncing || autoSyncing
        ? t('saves.syncing', 'Syncing saves…')
        : pending
          ? t('saves.pendingHint', 'Not in the cloud yet: uploaded once you are online')
          : t('saves.syncMenu', 'Sync saves')}
    disabled={!ready && !folder}
    aria-haspopup="menu"
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    {#if syncing || autoSyncing}
      <span class="spinner"></span>
    {:else}
      <svg viewBox="0 0 24 24"><path d="M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM8 3v5h7V3M7 21v-7h10v7" /></svg>
      {#if pending}<span class="dot"></span>{/if}
    {/if}
  </button>
  {#if open}
    <div class="menu" role="menu">
      {#if pending}
        <p class="note">{t('saves.pendingHint', 'Not in the cloud yet: uploaded once you are online')}</p>
        <div class="sep"></div>
      {/if}
      {#each items as item (item.mode)}
        <button role="menuitem" disabled={blocked || !ready} title={item.hint()} onclick={() => sync(item.mode)}>
          {#if item.mode === 'download'}
            <svg viewBox="0 0 24 24"><path d="M12 4v11M7 10.5l5 5 5-5M5 20h14" /></svg>
          {:else}
            <svg viewBox="0 0 24 24"><path d="M12 20V9M7 13.5l5-5 5 5M5 4h14" /></svg>
          {/if}
          <span>{item.label()}</span>
        </button>
      {/each}
      <div class="sep"></div>
      <button
        role="menuitem"
        disabled={!folder}
        title={folder ? folder.path : t('saves.notYet', 'The folder does not exist yet; it is created by the game or by the first download.')}
        onclick={openFolder}
      >
        <svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
        <span>{t('saves.openFolder', 'Saves')}</span>
      </button>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
  }
  svg {
    width: 20px;
    height: 20px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  /* sits next to the big Play button, same height */
  .icon {
    position: relative;
    width: 46px;
    height: 46px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    color: var(--text-secondary);
    background: var(--background-lighter);
    transition: filter var(--transition), transform var(--transition);
  }
  .icon:active:not(:disabled) {
    transform: scale(0.98);
  }
  .icon:hover:not(:disabled),
  .icon.active {
    color: var(--text-default);
    background: var(--current-background);
  }
  .icon:disabled {
    opacity: 0.4;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 20;
    min-width: 220px;
    padding: 6px;
    border-radius: 10px;
    background: var(--navbar-background);
    border: 1px solid var(--background-lighter);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-radius: 6px;
    text-align: left;
    font-size: 14px;
    color: var(--text-default);
  }
  .menu button svg {
    width: 18px;
    height: 18px;
    color: var(--text-secondary);
  }
  .menu button:hover:not(:disabled) {
    background: var(--current-background);
  }
  .menu button:disabled {
    opacity: 0.45;
  }
  .dot {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent-overlay);
  }
  .note {
    margin: 0;
    padding: 6px 10px;
    max-width: 240px;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--background-lighter);
  }
  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid var(--background-lighter);
    border-top-color: var(--accent-overlay);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
