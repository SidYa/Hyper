<script lang="ts">
  // Cloud saves sheet of the console game page: the manual sync actions of
  // the desktop CloudSavesMenu as a list for the controller. Owns input
  // while open (pushes its own handler).
  import { errorMessage, syncSaves } from '$lib/api/commands'
  import { gamepad, pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { notify } from '$lib/state/toast.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'
  import type { GameInfo, SaveSyncMode } from '$types/index'

  interface Props {
    game: GameInfo
    /** played offline: the cloud does not have these saves yet */
    pending?: boolean
    onclose: () => void
  }
  let { game, pending = false, onclose }: Props = $props()

  // the newer side wins either way; no forced overwrites (owner decision)
  const rows: Array<{ mode: SaveSyncMode; label: string; hint: string }> = [
    { mode: 'download', label: t('saves.download', 'Download'), hint: t('saves.downloadHint', 'Get newer saves from the cloud') },
    { mode: 'upload', label: t('saves.upload', 'Upload'), hint: t('saves.uploadHint', 'Send newer saves from this PC to the cloud') }
  ]

  let index = $state(0)
  let syncing = $state(false)

  function choose(i = index) {
    const row = rows[i]
    if (!row || syncing) return
    void sync(row.mode)
  }

  async function sync(mode: SaveSyncMode) {
    if (!game.runner) return
    syncing = true
    try {
      const summary = await syncSaves(game.runner, game.appName, mode)
      const lastLine = summary.split('\n').filter(Boolean).pop()
      notify(t('saves.synced', 'Saves of {{title}} synced', { title: game.title }) + (lastLine ? `: ${lastLine}` : ''), 'success', 6000)
      onclose()
    } catch (e) {
      notify(errorMessage(e), 'error', 9000)
    } finally {
      syncing = false
    }
  }

  $effect(() =>
    pushHandler((action) => {
      if (syncing) return true
      const dir = directionOf(action)
      if (dir === 'up') index = Math.max(0, index - 1)
      else if (dir === 'down') index = Math.min(rows.length - 1, index + 1)
      else if (action === 'mainAction') choose()
      else if (action === 'back') onclose()
      return true
    })
  )
</script>

<div class="overlay" data-focus-scope>
  <div class="card">
    <h2>{t('saves.syncMenu', 'Sync saves')}</h2>
    <p class="game">{game.title}</p>
    {#if pending}
      <p class="pending">{t('saves.pendingHint', 'Not in the cloud yet: uploaded once you are online')}</p>
    {/if}
    <div class="list">
      {#each rows as row, i (row.mode)}
        <button
          class="row"
          class:focused={i === index}
          disabled={syncing}
          onmouseenter={() => (index = i)}
          onclick={() => {
            index = i
            choose(i)
          }}
        >
          <span class="label">{row.label}</span>
          <span class="hint">{row.hint}</span>
        </button>
      {/each}
    </div>
    {#if syncing}
      <p class="status">{t('saves.syncing', 'Syncing saves…')}</p>
    {:else if gamepad.connected}
      <p class="hint-row">
        <span><ButtonGlyph button="main" layout={gamepad.layout} size={24} /> {t('console.select', 'Select')}</span>
        <span><ButtonGlyph button="back" layout={gamepad.layout} size={24} /> {t('console.back', 'Back')}</span>
      </p>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(5, 6, 15, 0.82);
    backdrop-filter: blur(8px);
    z-index: 20;
  }
  .card {
    width: min(640px, 86vw);
    padding: 32px 36px 24px;
    border-radius: 20px;
    background: rgba(255, 255, 255, 0.06);
  }
  h2 {
    font-size: 28px;
    margin: 0 0 6px;
  }
  .game {
    margin: 0 0 18px;
    font-size: 18px;
    color: var(--text-secondary);
  }
  .list {
    padding: 4px;
  }
  .row {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px 20px;
    margin-bottom: 8px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.04);
    color: var(--text-default);
    text-align: left;
    outline: none;
    transition: box-shadow 120ms ease, background 120ms ease;
  }
  .row.focused {
    background: rgba(255, 255, 255, 0.1);
    box-shadow: 0 0 0 3px var(--console-focus, #fff);
  }
  .label {
    font-size: 19px;
    font-weight: 600;
  }
  .hint {
    font-size: 15px;
    color: var(--text-secondary);
  }
  .pending {
    margin: -10px 0 16px;
    font-size: 16px;
    color: var(--accent-overlay);
  }
  .status {
    margin: 14px 0 0;
    text-align: center;
    color: var(--text-secondary);
  }
  .hint-row {
    display: flex;
    justify-content: center;
    gap: 28px;
    margin: 14px 0 0;
  }
  .hint-row span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
</style>
