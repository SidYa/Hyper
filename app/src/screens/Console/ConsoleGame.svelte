<script lang="ts">
  // Console game page (Start on a tile), modelled on Steam Big Picture: key
  // art with the logo, an action bar (play / install, update, cloud saves,
  // uninstall; the last two as icons only),
  // play time, a screenshot strip with a full-screen viewer and
  // the store description. The parent owns the launch / install / cancel
  // overlays; while one of them is up (`active` false) input falls through.
  import { onMount, tick, untrack } from 'svelte'
  import { gameStatus, library, savesPending, storeOf, uninstall, update } from '$lib/state/library.svelte'
  import { host } from '$lib/state/host.svelte'
  import { t, i18n } from '$lib/i18n/index.svelte'
  import { gamepad, pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { errorMessage, getAchievements, getCloudSaves, getDlcs, getGameExtra, getInstallInfo, getPlayStats } from '$lib/api/commands'
  import { settings } from '$lib/state/settings.svelte'
  import { formatBytes, formatPlaytime, plainText } from '$lib/format'
  import { heroImage, runnerName, screenshotUrl } from '$lib/library/view'
  import ControllerHints from '$lib/components/ControllerHints.svelte'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'
  import MorphLabel from '$lib/components/MorphLabel.svelte'
  import type { Achievements, CloudSaves, DlcEntry, GameExtra, GameInfo, InstallInfo, PlayStats } from '$types/index'
  import ConsoleClock from './ConsoleClock.svelte'
  import ConsoleDialog from './ConsoleDialog.svelte'
  import ConsoleSaves from './ConsoleSaves.svelte'
  import ConsoleDlcs from './ConsoleDlcs.svelte'

  interface Props {
    game: GameInfo
    /** false while a parent overlay (launch, install, cancel…) owns input */
    active: boolean
    onplay: (game: GameInfo) => void
    onback: () => void
  }
  let { game: initial, active, onplay, onback }: Props = $props()

  // follow library reloads (install finished, update applied…)
  const game = $derived(library.games.find((g) => g.runner === initial.runner && g.appName === initial.appName) ?? initial)
  const status = $derived(gameStatus(game))
  const busy = $derived(!!status && status.status !== 'done' && status.status !== 'error')
  const downloading = $derived(['queued', 'installing', 'updating', 'repairing'].includes(status?.status ?? ''))
  const paused = $derived(status?.status === 'paused')
  const moving = $derived(status?.status === 'moving')
  // added by hand: "uninstall" only forgets the game
  const sideload = $derived(game.runner === 'sideload')

  let extra = $state<GameExtra | null>(null)
  let extraLoading = $state(true)
  let stats = $state<PlayStats | null>(null)
  // Download / disk size of a game not installed yet (the backend caches it),
  // as on the desktop game page
  let info = $state<InstallInfo | null>(null)
  let infoError = $state<string | null>(null)
  const infoKey = $derived(
    !game.isInstalled && game.installable && !game.thirdPartyManagedApp && game.runner && storeOf(game.runner)?.loggedIn ? `${game.runner}:${game.appName}` : ''
  )
  $effect(() => {
    const key = infoKey
    // the key, not the game: library reloads must not refetch
    const g = untrack(() => game)
    info = null
    infoError = null
    if (!key || !g.runner) return
    getInstallInfo(g.runner, g.appName)
      .then((i) => {
        if (key === infoKey) info = i
      })
      .catch((e) => {
        if (key === infoKey) infoError = errorMessage(e)
      })
  })
  let saves = $state<CloudSaves | null>(null)
  let savesOpen = $state(false)

  onMount(() => {
    if (!game.runner) return
    getGameExtra(game.runner, game.appName)
      .then((e) => (extra = e))
      .catch(() => (extra = null))
      .finally(() => (extraLoading = false))
  })
  // cloud saves of an installed store game (on Linux: folders inside its Wine prefix)
  const savesKey = $derived(game.isInstalled && !sideload ? `${game.runner}:${game.appName}` : '')
  $effect(() => {
    if (!savesKey || !game.runner) return void (saves = null)
    getCloudSaves(game.runner, game.appName)
      .then((c) => (saves = c))
      .catch(() => (saves = null))
  })
  const savesReady = $derived(!!saves?.supported && saves.locations.length > 0 && saves.locations.every((l) => l.path))

  // achievements of a store game (the account's progress); none → no section
  let achievements = $state<Achievements | null>(null)
  const achKey = $derived(!sideload && game.runner && storeOf(game.runner)?.loggedIn ? `${game.runner}:${game.appName}` : '')
  $effect(() => {
    const key = achKey
    const g = untrack(() => game)
    achievements = null
    if (!key || !g.runner) return
    getAchievements(g.runner, g.appName)
      .then((a) => {
        if (key === achKey) achievements = a
      })
      .catch(() => (achievements = null))
  })
  const achDate = (iso?: string) => (iso ? new Date(iso).toLocaleDateString(i18n.locale, { day: 'numeric', month: 'short', year: 'numeric' }) : '')


  // add-ons of an installed store game; re-read when the library reloads
  let dlcs = $state<DlcEntry[]>([])
  let dlcOpen = $state(false)
  $effect(() => {
    const g = game
    void library.games
    if (!g.runner || !g.isInstalled || g.isDlc || sideload || g.thirdPartyManagedApp) return void (dlcs = [])
    getDlcs(g.runner, g.appName)
      .then((d) => (dlcs = d))
      .catch(() => (dlcs = []))
  })

  // re-read play time whenever the game stops running; keyed on the status
  // string, not the status object: progress events must not refetch
  const playing = $derived(status?.status === 'playing')
  const statsKey = $derived(`${game.runner}:${game.appName}:${playing}`)
  $effect(() => {
    const key = statsKey
    const g = untrack(() => game)
    if (!g.runner || untrack(() => playing)) return
    getPlayStats(g.runner, g.appName)
      .then((s) => {
        if (key === statsKey) stats = s
      })
      .catch(() => {})
  })

  const description = $derived(plainText(extra?.description ?? game.description ?? ''))
  const screenshots = $derived(extra?.screenshots ?? [])

  type ActionId = 'primary' | 'update' | 'saves' | 'dlc' | 'uninstall'
  const primaryLabel = $derived.by(() => {
    if (status?.status === 'playing') return t('console.game.close', 'Close game')
    if (downloading) return t('console.cancelDownload', 'Cancel download')
    if (paused) return t('console.game.resumeDownload', 'Resume download')
    if (moving) return t('game.moving', 'Moving…')
    if (status?.status === 'preparing') return t('game.preparing', 'Preparing')
    if (status?.status === 'launching') return t('game.launching', 'Launching')
    if (status?.status === 'syncing') return t('saves.syncing', 'Syncing saves…')
    if (!game.isInstalled) return t('console.install', 'Install')
    return t('console.play', 'Play')
  })
  // iconOnly: the label is the button's tooltip / accessible name
  const actions = $derived<Array<{ id: ActionId; label: string; icon: string; danger?: boolean; iconOnly?: boolean }>>([
    { id: 'primary', label: primaryLabel, icon: '' },
    ...(game.isInstalled && game.updateAvailable && !busy
      ? [{ id: 'update' as const, label: t('game.update', 'Update'), icon: 'M12 20V8m-5 5 5-5 5 5M5 4h14' }]
      : []),
    ...(savesReady && !busy
      ? [{
          id: 'saves' as const,
          label: t('saves.syncMenu', 'Sync saves'),
          icon: 'M5 3h11l5 5v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM8 3v5h7V3M7 21v-7h10v7',
          iconOnly: true
        }]
      : []),
    ...(dlcs.length && game.isInstalled && !busy
      ? [{
          id: 'dlc' as const,
          label: t('dlc.title', 'Downloadable content'),
          icon: 'M4 7h16v4H4zM6 11v9h12v-9M10 7V4h4v3M10 15h4',
          iconOnly: true
        }]
      : []),
    ...(game.isInstalled && !busy
      ? [{
          id: 'uninstall' as const,
          label: sideload ? t('sideload.remove', 'Remove from library') : t('game.uninstall', 'Uninstall'),
          icon: 'M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13',
          danger: true,
          iconOnly: true
        }]
      : [])
  ])

  type Zone = 'actions' | 'shots' | 'achievements' | 'about'
  let zone = $state<Zone>('actions')
  let actionIndex = $state(0)
  let shotIndex = $state(0)
  let viewer = $state<number | null>(null)
  let confirmUninstall = $state(false)

  let bodyEl = $state<HTMLDivElement | undefined>()
  let shotEls = $state<Array<HTMLButtonElement | undefined>>([])
  let aboutEl = $state<HTMLElement | undefined>()
  let achEl = $state<HTMLElement | undefined>()

  $effect(() => {
    if (actionIndex >= actions.length) actionIndex = actions.length - 1
  })

  const zones = $derived<Zone[]>([
    'actions',
    ...(screenshots.length ? (['shots'] as Zone[]) : []),
    ...(description || extra ? (['about'] as Zone[]) : []),
    ...(achievements && achievements.total > 0 ? (['achievements'] as Zone[]) : [])
  ])

  // Key-repeat scrolling of the body: each press moves the goal, not the
  // current (mid-animation) position, or holding Down crawls and stutters
  // because every smooth scroll restarts from wherever the last one got to.
  let scrollGoal: number | null = null
  let scrollGoalAt = 0
  function scrollFrom(): number {
    const box = bodyEl
    if (!box) return 0
    // a goal older than the smooth scroll it started is stale (wheel, zone change)
    return scrollGoal !== null && performance.now() - scrollGoalAt < 500 ? scrollGoal : box.scrollTop
  }
  function scrollBodyTo(top: number) {
    const box = bodyEl
    if (!box) return
    scrollGoal = Math.max(0, Math.min(box.scrollHeight - box.clientHeight, top))
    scrollGoalAt = performance.now()
    box.scrollTo({ top: scrollGoal, behavior: 'smooth' })
  }

  function reveal() {
    scrollGoal = null
    if (zone === 'actions') bodyEl?.scrollTo({ top: 0, behavior: 'smooth' })
    else if (zone === 'shots') {
      shotEls[shotIndex]?.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'smooth' })
    } else if (zone === 'achievements') achEl?.scrollIntoView({ block: 'start', behavior: 'smooth' })
    else aboutEl?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  }
  $effect(() => {
    void zone
    void shotIndex
    tick().then(reveal)
  })

  function run(id: ActionId) {
    if (id === 'primary') {
      if (status?.status !== 'launching' && status?.status !== 'preparing') onplay(game)
    }
    else if (id === 'update') void update(game)
    else if (id === 'saves') savesOpen = true
    else if (id === 'dlc') dlcOpen = true
    else if (id === 'uninstall') confirmUninstall = true
  }

  // a sideloaded game is gone from the library afterwards: leave its page
  async function removeGame() {
    if ((await uninstall(game)) && sideload) onback()
  }

  function stepZone(delta: 1 | -1) {
    const i = zones.indexOf(zone) + delta
    if (i >= 0 && i < zones.length) zone = zones[i]
  }

  $effect(() =>
    pushHandler((action) => {
      if (!active || savesOpen || dlcOpen) return false
      const dir = directionOf(action)

      if (confirmUninstall) {
        if (action === 'mainAction') {
          confirmUninstall = false
          void removeGame()
        } else if (action === 'back') confirmUninstall = false
        return true
      }
      if (viewer !== null) {
        if (dir === 'left') viewer = Math.max(0, viewer - 1)
        else if (dir === 'right') viewer = Math.min(screenshots.length - 1, viewer + 1)
        else if (action === 'back' || action === 'mainAction') {
          shotIndex = viewer
          viewer = null
        }
        return true
      }

      if (action === 'back') onback()
      else if (zone === 'actions') {
        if (dir === 'left') actionIndex = Math.max(0, actionIndex - 1)
        else if (dir === 'right') actionIndex = Math.min(actions.length - 1, actionIndex + 1)
        else if (dir === 'down') stepZone(1)
        else if (action === 'mainAction') run(actions[actionIndex].id)
      } else if (zone === 'shots') {
        if (dir === 'left') shotIndex = Math.max(0, shotIndex - 1)
        else if (dir === 'right') shotIndex = Math.min(screenshots.length - 1, shotIndex + 1)
        else if (dir === 'up') stepZone(-1)
        else if (dir === 'down') stepZone(1)
        else if (action === 'mainAction') viewer = shotIndex
      } else {
        // the description and the achievements scroll; Up at the top goes to the zone above,
        // Down past the end of the description to the achievements (below it, as on the desktop page)
        const box = bodyEl
        const el = zone === 'achievements' ? achEl : aboutEl
        const top = el ? el.offsetTop - 24 : 0
        const from = scrollFrom()
        if (dir === 'down' && box) {
          const bottom = el ? el.offsetTop + el.offsetHeight : 0
          if (zone === 'about' && from + box.clientHeight >= bottom - 4) stepZone(1)
          else scrollBodyTo(from + 220)
        } else if (dir === 'up' && box) {
          if (from > top + 4) scrollBodyTo(from - 220)
          else stepZone(-1)
        }
      }
      return true
    })
  )

  const lastPlayed = $derived(
    stats?.lastPlayed
      ? new Date(stats.lastPlayed).toLocaleDateString(i18n.locale, { day: 'numeric', month: 'long', year: 'numeric' })
      : null
  )
  const releaseDate = $derived(
    extra?.releaseDate ? new Date(extra.releaseDate).toLocaleDateString(i18n.locale, { day: 'numeric', month: 'long', year: 'numeric' }) : null
  )
  const storeName = $derived(runnerName(game.runner))

  const hints = $derived(
    viewer !== null
      ? [
          { buttons: ['dpad' as const], label: t('console.game.browse', 'Browse') },
          { buttons: ['back' as const], label: t('console.back', 'Back') }
        ]
      : [
          { buttons: ['main' as const], label: zone === 'shots' ? t('console.game.view', 'View') : t('console.select', 'Select') },
          { buttons: ['back' as const], label: t('console.back', 'Back') },
          { buttons: ['dpad' as const], label: t('console.navigate', 'Navigate') }
        ]
  )
</script>

<div class="game">
  <div class="body" bind:this={bodyEl}>
    <section class="hero">
      {#if heroImage(game)}<img class="art" src={heroImage(game)} alt="" />{/if}
      <div class="fade"></div>
      <div class="clock"><ConsoleClock /></div>
      <div class="head">
        {#if game.artLogo}
          <img class="logo" src={game.artLogo} alt={game.title} />
        {:else}
          <h1>{game.title}</h1>
        {/if}
      </div>
    </section>

    <section class="bar">
      <div class="actions">
        {#each actions as a, i (a.id)}
          <button
            class="act"
            class:primary={a.id === 'primary'}
            class:danger={a.danger}
            class:icon-only={a.iconOnly}
            class:focused={zone === 'actions' && i === actionIndex}
            title={a.iconOnly ? a.label : undefined}
            aria-label={a.iconOnly ? a.label : undefined}
            onmouseenter={() => {
              zone = 'actions'
              actionIndex = i
            }}
            onclick={() => run(a.id)}
          >
            {#if a.id === 'primary'}
              {#if zone === 'actions' && i === actionIndex}<ButtonGlyph button="main" layout={gamepad.layout} size={26} />{/if}
              <MorphLabel text={a.label} busy={status?.status === 'preparing' || status?.status === 'launching' || status?.status === 'syncing'} />
            {:else}
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d={a.icon} /></svg>
              {#if !a.iconOnly}<span>{a.label}</span>{/if}
            {/if}
          </button>
        {/each}
      </div>

      <dl class="stats">
        {#if (downloading || moving) && status?.progress}
          <div><dt>{moving ? t('game.moving', 'Moving…') : t('console.game.progress', 'Progress')}</dt><dd>{status.progress.percent.toFixed(0)}%</dd></div>
        {/if}
        <div>
          <dt>{t('console.game.playTime', 'Play time')}</dt>
          <dd>{stats ? formatPlaytime(stats.totalSecs, { h: t('common.hoursShort', 'h'), min: t('common.minutesShort', 'min') }) : '—'}</dd>
        </div>
        <div><dt>{t('console.game.lastPlayed', 'Last played')}</dt><dd>{lastPlayed ?? t('console.game.never', 'Never')}</dd></div>
        {#if game.install?.installSize}
          <div><dt>{t('game.diskSize', 'On disk')}</dt><dd>{game.install.installSize}</dd></div>
        {:else if infoKey}
          <div>
            <dt>{t('game.diskSize', 'On disk')}</dt>
            <dd>
              {#if info}{formatBytes(info.diskSize)}
              {:else if infoError}<span class="err" title={infoError}>—</span>
              {:else}<span class="calc"><span class="spinner"></span>{t('game.calculating', 'Calculating size…')}</span>{/if}
            </dd>
          </div>
          {#if info}
            <div><dt>{t('game.downloadSize', 'Download')}</dt><dd>{formatBytes(info.downloadSize)}</dd></div>
          {/if}
          <div><dt>{t('game.installPath', 'Install to')}</dt><dd class="path">{settings.value.defaultInstallPath}</dd></div>
        {/if}
      </dl>
      {#if (downloading || moving) && status?.progress}
        <span class="progress"><span class="fill" style:width={`${status.progress.percent}%`}></span></span>
      {/if}
    </section>

    {#if screenshots.length}
      <section class="shots-section">
        <h2>{t('console.game.screenshots', 'Screenshots')}</h2>
        <div class="shots">
          {#each screenshots as src, i (src)}
            <button
              class="shot"
              class:focused={zone === 'shots' && i === shotIndex}
              bind:this={shotEls[i]}
              onclick={() => {
                zone = 'shots'
                shotIndex = i
                viewer = i
              }}
            >
              <img src={screenshotUrl(src, 640)} alt="" loading="lazy" decoding="async" />
            </button>
          {/each}
        </div>
      </section>
    {:else if extraLoading}
      <section class="shots-section"><div class="shots"><span class="shot ph"></span><span class="shot ph"></span><span class="shot ph"></span></div></section>
    {/if}

    {#if description || extra}
      <section class="about" class:focused={zone === 'about'} bind:this={aboutEl}>
        <div class="desc">
          <h2>{t('console.game.about', 'About')}</h2>
          {#if description}
            {#each description.split(/\n{2,}/) as para, i (i)}<p>{para}</p>{/each}
          {:else}
            <p class="muted">{t('game.noDescription', 'No description available.')}</p>
          {/if}
        </div>
        <aside>
          {#if extra?.developer ?? game.developer}<div><dt>{t('console.game.developer', 'Developer')}</dt><dd>{extra?.developer ?? game.developer}</dd></div>{/if}
          {#if extra?.publisher}<div><dt>{t('console.game.publisher', 'Publisher')}</dt><dd>{extra.publisher}</dd></div>{/if}
          {#if releaseDate}<div><dt>{t('console.game.released', 'Release date')}</dt><dd>{releaseDate}</dd></div>{/if}
          {#if storeName}<div><dt>{t('console.game.store', 'Store')}</dt><dd>{storeName}</dd></div>{/if}
          {#if extra?.tags.length}
            <div class="tags">{#each extra.tags as tag (tag)}<span>{tag}</span>{/each}</div>
          {/if}
        </aside>
      </section>
    {/if}

    {#if achievements && achievements.total > 0}
      <section class="ach-section" class:focused={zone === 'achievements'} bind:this={achEl}>
        <h2>
          {t('achievements.title', 'Achievements')}
          <span class="ach-count">{t('achievements.unlocked', '{{done}} of {{total}} unlocked', { done: achievements.unlocked, total: achievements.total })}</span>
        </h2>
        <div class="ach-grid">
          {#each achievements.list as a (a.id)}
            <div class="ach" class:unlocked={a.unlocked}>
              <img src={a.unlocked ? a.iconUnlocked : a.iconLocked} alt="" loading="lazy" />
              <span class="ach-text">
                <span class="ach-name">{a.hidden ? t('achievements.hidden', 'Hidden achievement') : a.name}</span>
                <span class="ach-desc">{a.hidden ? '' : a.description}</span>
                <span class="ach-meta">{a.unlocked ? achDate(a.unlockedAt) : a.rarityLabel}{a.rarity ? ` · ${a.rarity.toFixed(1)}%` : ''}</span>
              </span>
            </div>
          {/each}
        </div>
      </section>
    {/if}
  </div>

  <footer>
    {#if gamepad.connected && active}<ControllerHints layout={gamepad.layout} {hints} />{/if}
  </footer>

  {#if viewer !== null}
    <div class="viewer" role="dialog">
      <img src={screenshotUrl(screenshots[viewer], 1920)} alt="" />
      <span class="count">{viewer + 1} / {screenshots.length}</span>
      <button class="close" onclick={() => (viewer = null)} aria-label={t('console.back', 'Back')}>✕</button>
    </div>
  {/if}

  {#if savesOpen}
    <ConsoleSaves {game} pending={savesPending(game, saves?.pendingUpload)} onclose={() => (savesOpen = false)} />
  {/if}

  {#if dlcOpen}
    <ConsoleDlcs {game} {dlcs} onclose={() => (dlcOpen = false)} />
  {/if}

  {#if confirmUninstall}
    <ConsoleDialog
      title={sideload ? t('sideload.removeQ', 'Remove from the library?') : t('console.game.uninstallQ', 'Uninstall the game?')}
      gameTitle={game.title}
      message={sideload ? t('sideload.removeMessage', 'The game files stay on disk; only the card is removed.') : game.thirdPartyManagedApp ? t('game.thirdPartyUninstall', 'Only the entry here is removed; uninstall the game itself in {{app}}.', { app: game.thirdPartyManagedApp }) : host.linux ? t('console.game.uninstallMessageLinux', 'The game files and its Wine prefix (settings and local saves) will be deleted from this PC.') : t('console.game.uninstallMessage', 'The game files will be deleted from this PC.')}
      confirmLabel={sideload ? t('sideload.remove', 'Remove from library') : t('game.uninstall', 'Uninstall')}
      cancelLabel={t('console.keep', 'Keep')}
      danger
      onconfirm={() => {
        confirmUninstall = false
        void removeGame()
      }}
      oncancel={() => (confirmUninstall = false)}
    />
  {/if}
</div>

<style>
  .game {
    position: absolute;
    inset: 0;
    z-index: 20;
    display: grid;
    grid-template-rows: 1fr auto;
    background: var(--body-background);
  }
  .body {
    min-height: 0;
    overflow-y: auto;
    scrollbar-width: none;
    padding-bottom: 40px;
  }
  .hero {
    position: relative;
    height: 52vh;
    min-height: 320px;
    overflow: hidden;
  }
  .art {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .fade {
    position: absolute;
    inset: 0;
    background:
      linear-gradient(to top, var(--body-background) 0%, transparent 55%),
      linear-gradient(to right, rgba(0, 0, 0, 0.45) 0%, transparent 50%);
  }
  .clock {
    position: absolute;
    top: 28px;
    right: 56px;
    padding: 4px 12px;
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.35);
  }
  .head {
    position: absolute;
    left: 56px;
    right: 56px;
    bottom: 28px;
  }
  .logo {
    max-width: min(520px, 50vw);
    max-height: 150px;
    object-fit: contain;
    object-position: left bottom;
    filter: drop-shadow(0 4px 18px rgba(0, 0, 0, 0.6));
  }
  h1 {
    margin: 0;
    font-size: 52px;
    font-weight: 700;
    text-shadow: 0 4px 18px rgba(0, 0, 0, 0.6);
  }
  .bar {
    position: relative;
    display: flex;
    align-items: center;
    gap: 40px;
    flex-wrap: wrap;
    padding: 8px 56px 24px;
  }
  .actions {
    display: flex;
    gap: 12px;
    padding: 4px;
  }
  .act {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    height: 60px;
    padding: 0 22px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-default);
    font-size: 18px;
    font-weight: 500;
    outline: none;
    transition: transform 120ms ease, box-shadow 120ms ease, background 120ms ease;
  }
  .act.primary {
    min-width: 220px;
    justify-content: center;
    padding: 0 32px;
    background: var(--accent);
    color: var(--accent-text);
    font-size: 21px;
    font-weight: 600;
  }
  .act.focused {
    transform: scale(1.04);
    box-shadow: 0 0 0 3px var(--console-focus);
  }
  .act:not(.primary).focused {
    background: rgba(255, 255, 255, 0.16);
  }
  .act.danger.focused {
    color: var(--danger);
  }
  .act.icon-only {
    width: 60px;
    padding: 0;
    justify-content: center;
  }
  .act svg {
    width: 24px;
    height: 24px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .stats {
    display: flex;
    gap: 36px;
    margin: 0;
  }
  dt {
    font-size: 14px;
    color: var(--text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  dd {
    margin: 4px 0 0;
    font-size: 19px;
    font-weight: 500;
  }
  .calc {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 400;
    color: var(--text-secondary);
  }
  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid rgba(255, 255, 255, 0.2);
    border-top-color: var(--accent-overlay);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .err {
    color: var(--danger);
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .progress {
    position: absolute;
    left: 56px;
    right: 56px;
    bottom: 10px;
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.12);
    overflow: hidden;
  }
  .progress .fill {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 400ms ease;
  }
  h2 {
    margin: 0 0 16px;
    font-size: 22px;
    font-weight: 600;
  }
  .shots-section {
    padding: 16px 56px 8px;
  }
  .shots {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    scrollbar-width: none;
    /* the strip clips what overflows it: room for the focused tile's
       scale-up and ring, pulled back by the margin so the tiles stay put */
    padding: 14px 20px 20px;
    margin: -14px -20px -20px;
    scroll-padding-inline: 20px;
  }
  .shot {
    flex: none;
    width: 384px;
    aspect-ratio: 16 / 9;
    padding: 0;
    border: 0;
    border-radius: 10px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.06);
    outline: none;
    transition: transform 120ms ease, box-shadow 120ms ease;
  }
  /* rounded itself too: a scaled (composited) card does not always clip
     its image to the radius */
  .shot img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    border-radius: inherit;
  }
  .shot.focused {
    transform: scale(1.04);
    box-shadow: 0 0 0 3px var(--console-focus);
  }
  .shot.ph {
    animation: pulse 1.2s ease-in-out infinite alternate;
  }
  .about {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 320px;
    gap: 48px;
    margin: 24px 50px 0;
    padding: 24px 6px 24px 6px;
    border-radius: 14px;
    transition: background 140ms ease;
  }
  .about.focused {
    background: rgba(255, 255, 255, 0.04);
  }
  .desc {
    max-width: 960px;
    padding-left: 18px;
  }
  .desc p {
    margin: 0 0 14px;
    font-size: 18px;
    line-height: 1.6;
    color: var(--text-secondary);
    white-space: pre-line;
  }
  aside {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .tags span {
    padding: 4px 12px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    font-size: 14px;
    color: var(--text-secondary);
  }
  footer {
    display: flex;
    justify-content: center;
    min-height: 56px;
    padding: 10px 56px 22px;
  }
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 30;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.92);
    animation: fade 160ms ease-out;
  }
  .viewer img {
    max-width: 94vw;
    max-height: 88vh;
    border-radius: 16px;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.6);
  }
  .count {
    position: absolute;
    bottom: 28px;
    font-size: 18px;
    color: var(--text-secondary);
  }
  .close {
    position: absolute;
    top: 24px;
    right: 32px;
    font-size: 26px;
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
  /* achievements */
  .ach-section {
    margin: 8px 50px 0;
    padding: 16px 6px 24px;
    border-radius: 14px;
    transition: background 140ms ease;
  }
  .ach-section.focused {
    background: rgba(255, 255, 255, 0.04);
  }
  .ach-section h2 {
    display: flex;
    align-items: baseline;
    gap: 16px;
    margin: 0 0 14px;
    font-size: 22px;
  }
  .ach-count {
    font-size: 16px;
    font-weight: 400;
    color: var(--text-secondary);
  }
  .ach-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 10px;
  }
  .ach {
    display: flex;
    gap: 14px;
    align-items: center;
    padding: 10px 14px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.05);
    min-width: 0;
  }
  .ach:not(.unlocked) {
    opacity: 0.55;
  }
  .ach:not(.unlocked) img {
    filter: grayscale(1);
  }
  .ach img {
    width: 56px;
    height: 56px;
    flex-shrink: 0;
    border-radius: 10px;
    object-fit: cover;
    background: rgba(255, 255, 255, 0.08);
  }
  .ach-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    font-size: 15px;
  }
  .ach-name {
    font-weight: 600;
    font-size: 17px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ach-desc {
    color: var(--text-secondary);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .ach-meta {
    color: var(--text-tertiary);
    font-size: 13px;
  }
</style>
