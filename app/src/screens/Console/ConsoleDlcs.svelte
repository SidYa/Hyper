<script lang="ts">
  // DLC sheet of the console game page: the game's add-ons as a list; A
  // installs one (into the game's folder) or, after a confirmation,
  // uninstalls it. Owns input while open (pushes its own handler).
  import { gameStatus, install, uninstall } from '$lib/state/library.svelte'
  import { gamepad, pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { t } from '$lib/i18n/index.svelte'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'
  import ConsoleDialog from './ConsoleDialog.svelte'
  import type { DlcEntry, GameInfo } from '$types/index'

  interface Props {
    game: GameInfo
    dlcs: DlcEntry[]
    onclose: () => void
  }
  let { game, dlcs, onclose }: Props = $props()

  let index = $state(0)
  let confirm = $state<DlcEntry | null>(null)

  // a DLC is a game of its own to the backend (install / status by its app name)
  const asGame = (d: DlcEntry): GameInfo => ({ ...game, appName: d.appName, title: d.title, isDlc: true })
  const stateOf = (d: DlcEntry) => gameStatus(asGame(d))

  function choose(i = index) {
    const d = dlcs[i]
    if (!d) return
    const s = stateOf(d)
    if (s && s.status !== 'done' && s.status !== 'error') return
    if (d.installed) confirm = d
    else if (d.installable) void install(asGame(d))
  }

  $effect(() =>
    pushHandler((action) => {
      const dir = directionOf(action)
      if (confirm) {
        if (action === 'mainAction') {
          void uninstall(asGame(confirm))
          confirm = null
        } else if (action === 'back') confirm = null
        return true
      }
      if (dir === 'up') index = Math.max(0, index - 1)
      else if (dir === 'down') index = Math.min(dlcs.length - 1, index + 1)
      else if (action === 'mainAction') choose()
      else if (action === 'back') onclose()
      return true
    })
  )

  function stateLabel(d: DlcEntry): string {
    const s = stateOf(d)
    if (s?.status === 'queued') return t('downloads.queued', 'Queued')
    if (s?.status === 'paused') return t('downloads.paused', 'Paused')
    if (s?.status === 'installing') return s.progress ? `${Math.floor(s.progress.percent)}%` : t('game.installing', 'Installing')
    if (s?.status === 'uninstalling') return t('game.uninstalling', 'Uninstalling…')
    if (d.installed) return t('dlc.installed', 'Installed')
    return d.installable ? t('dlc.notInstalled', 'Not installed') : t('dlc.withGame', 'Comes with the game')
  }
</script>

<div class="overlay" data-focus-scope>
  <div class="card">
    <h2>{t('dlc.title', 'Downloadable content')}</h2>
    <p class="game">{game.title}</p>
    <div class="list">
      {#each dlcs as d, i (d.appName)}
        <button
          class="row"
          class:focused={i === index}
          class:installed={d.installed}
          onmouseenter={() => (index = i)}
          onclick={() => {
            index = i
            choose(i)
          }}
        >
          <span class="label">{d.title}</span>
          <span class="hint">{stateLabel(d)}</span>
        </button>
      {/each}
    </div>
    {#if gamepad.connected}
      <p class="hint-row">
        <span><ButtonGlyph button="main" layout={gamepad.layout} size={24} /> {dlcs[index]?.installed ? t('game.uninstall', 'Uninstall') : t('console.install', 'Install')}</span>
        <span><ButtonGlyph button="back" layout={gamepad.layout} size={24} /> {t('console.back', 'Back')}</span>
      </p>
    {/if}
  </div>
</div>

{#if confirm}
  <ConsoleDialog
    title={t('dlc.uninstallQ', 'Uninstall this add-on?')}
    gameTitle={confirm.title}
    message={t('dlc.uninstallMessage', 'Its files are removed from the game folder; the game itself stays.')}
    confirmLabel={t('game.uninstall', 'Uninstall')}
    cancelLabel={t('console.keep', 'Keep')}
    danger
    onconfirm={() => {
      const d = confirm
      confirm = null
      if (d) void uninstall(asGame(d))
    }}
    oncancel={() => (confirm = null)}
  />
{/if}

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
    width: min(720px, 88vw);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
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
    overflow-y: auto;
  }
  .row {
    width: 100%;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
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
    flex-shrink: 0;
    font-size: 15px;
    color: var(--text-secondary);
  }
  .row.installed .hint {
    color: var(--console-accent, #8b7cf6);
  }
  .hint-row {
    display: flex;
    gap: 28px;
    margin: 14px 0 0;
    font-size: 15px;
    color: var(--text-secondary);
  }
  .hint-row span {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
</style>
