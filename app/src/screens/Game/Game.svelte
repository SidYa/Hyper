<script lang="ts">
  // Game page, laid out like the Steam library: full-bleed key art with the
  // logo, an action bar (big Play / Install, stats, tools) and the details
  // below in two columns.
  import { untrack } from 'svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { askConfirm } from '$lib/state/confirm.svelte'
  import { cancel, findGame, gameStatus, install, launch, library, repair, resume, statusKey, storeOf, uninstall, update } from '$lib/state/library.svelte'
  import { settings } from '$lib/state/settings.svelte'
  import { host } from '$lib/state/host.svelte'
  import { back } from '$lib/router.svelte'
  import { t, i18n } from '$lib/i18n/index.svelte'
  import { errorMessage, getAchievements, getCloudSaves, getDlcs, getGameExtra, getGameSettings, getInstallInfo, importGame, killGame, moveGame, openGameFolder, openExternal, saveGameSettings } from '$lib/api/commands'
  import { formatBytes, formatEta, formatSpeed, plainText } from '$lib/format'
  import { heroImage, runnerName } from '$lib/library/view'
  import { notify } from '$lib/state/toast.svelte'
  import AddGameDialog from '$lib/components/AddGameDialog.svelte'
  import CloudSavesMenu from '$lib/components/CloudSavesMenu.svelte'
  import MorphLabel from '$lib/components/MorphLabel.svelte'
  import ScreenshotGallery from '$lib/components/ScreenshotGallery.svelte'
  import type { Achievements, CloudSaves, DlcEntry, GameExtra, GameInfo, GameSettings, InstallInfo } from '$types/index'

  interface Props {
    runner: string
    appName: string
  }
  let { runner, appName }: Props = $props()

  const game = $derived(findGame(runner, appName))
  const status = $derived(game ? gameStatus(game) : undefined)
  const downloading = $derived(status?.status === 'installing' || status?.status === 'updating' || status?.status === 'queued' || status?.status === 'paused')
  const busy = $derived(!!status && status.status !== 'playing')
  const playing = $derived(status?.status === 'playing')
  // Play turns into what the launch is doing (Linux: "Preparing" on a first start)
  const moving = $derived(status?.status === 'moving')
  const starting = $derived(status?.status === 'preparing' || status?.status === 'launching' || moving)
  const playLabel = $derived(
    status?.status === 'preparing'
      ? t('game.preparing', 'Preparing')
      : status?.status === 'launching'
        ? t('game.launching', 'Launching')
        : moving
          ? t('game.moving', 'Moving…')
          : t('console.play', 'Play')
  )

  const hero = $derived(game ? heroImage(game) : '')
  // added by hand: edit instead of verify, "uninstall" only forgets it
  const sideload = $derived(game?.runner === 'sideload')
  let editing = $state(false)

  const report = (e: unknown) => notify(errorMessage(e), 'error')
  function openFolder() {
    if (game?.runner) openGameFolder(game.runner, game.appName).catch(report)
  }

  let installPath = $state('')
  let info = $state<InstallInfo | null>(null)
  let infoError = $state<string | null>(null)
  let logoBroken = $state(false)
  let importing = $state(false)

  $effect(() => {
    if (!installPath && settings.loaded) installPath = settings.value.defaultInstallPath
  })

  // Download / disk size for not-installed games (legendary info, cached).
  $effect(() => {
    const g = game
    if (!g || g.isInstalled || !g.installable || g.thirdPartyManagedApp || !g.runner || !storeOf(g.runner)?.loggedIn) return
    info = null
    infoError = null
    getInstallInfo(g.runner, g.appName)
      .then((i) => (info = i))
      .catch((e) => (infoError = errorMessage(e)))
  })

  // Store-page details (description, screenshots, publisher…), as on the
  // console game page; fetched once per game, cached by the backend.
  let extra = $state<GameExtra | null>(null)
  let extraLoading = $state(false)
  const extraKey = $derived(game?.runner ? `${game.runner}:${game.appName}` : '')
  $effect(() => {
    const key = extraKey
    // the key, not the game: library reloads must not refetch
    const g = untrack(() => game)
    extra = null
    if (!key || !g?.runner) return
    extraLoading = true
    getGameExtra(g.runner, g.appName)
      .then((e) => {
        if (key === extraKey) extra = e
      })
      .catch((e) => console.warn('store details:', errorMessage(e)))
      .finally(() => {
        if (key === extraKey) extraLoading = false
      })
  })
  const description = $derived(plainText(extra?.description ?? game?.description ?? ''))
  const releaseDate = $derived(
    extra?.releaseDate ? new Date(extra.releaseDate).toLocaleDateString(i18n.locale, { day: 'numeric', month: 'long', year: 'numeric' }) : null
  )

  // Add-ons of an installed store game; re-read when the library reloads
  // (a DLC finished installing or was removed)
  let dlcs = $state<DlcEntry[]>([])
  $effect(() => {
    const g = game
    void library.games
    if (!g?.runner || !g.isInstalled || g.isDlc || sideload || g.thirdPartyManagedApp) return void (dlcs = [])
    getDlcs(g.runner, g.appName)
      .then((d) => (dlcs = d))
      .catch((e) => console.warn('dlcs:', errorMessage(e)))
  })
  // a DLC is a game of its own to the backend
  const dlcGame = (d: DlcEntry): GameInfo => ({ ...game!, appName: d.appName, title: d.title, isDlc: true })
  const dlcStatus = (d: DlcEntry) => (game?.runner ? library.statuses[statusKey(game.runner, d.appName)] : undefined)

  // GOG: branch / build chosen for the install (empty = public, latest)
  let branch = $state('')
  let buildId = $state('')
  const builds = $derived(info?.builds.filter((b) => (b.branch ?? '') === branch) ?? [])

  // Achievements of a store game (the account's progress), hidden when the game has none
  let achievements = $state<Achievements | null>(null)
  const achKey = $derived(game && !sideload && game.runner && storeOf(game.runner)?.loggedIn ? `${game.runner}:${game.appName}` : '')
  $effect(() => {
    const key = achKey
    const g = untrack(() => game)
    achievements = null
    if (!key || !g?.runner) return
    getAchievements(g.runner, g.appName)
      .then((a) => {
        if (key === achKey) achievements = a
      })
      .catch((e) => console.warn('achievements:', errorMessage(e)))
  })
  const achDate = (iso?: string) => (iso ? new Date(iso).toLocaleDateString(i18n.locale, { day: 'numeric', month: 'short', year: 'numeric' }) : '')

  // Launch options (per-game settings file); edited in place, saved on demand
  let gs = $state<GameSettings | null>(null)
  let gsEnv = $state('')
  let gsSaving = $state(false)
  const gsKey = $derived(game && !sideload && game.runner ? `${game.runner}:${game.appName}` : '')
  $effect(() => {
    const key = gsKey
    const g = untrack(() => game)
    gs = null
    if (!key || !g?.runner) return
    getGameSettings(g.runner, g.appName)
      .then((s) => {
        if (key !== gsKey) return
        gs = s
        gsEnv = s.env.map((e) => `${e.key}=${e.value}`).join('\n')
      })
      .catch((e) => console.warn('game settings:', errorMessage(e)))
  })
  async function saveLaunchOptions() {
    if (!gs || !game?.runner) return
    gsSaving = true
    const env = gsEnv
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean)
      .map((l) => {
        const i = l.indexOf('=')
        return i > 0 ? { key: l.slice(0, i).trim(), value: l.slice(i + 1) } : { key: l, value: '' }
      })
    try {
      await saveGameSettings(game.runner, game.appName, { ...gs, env })
      notify(t('launchOptions.saved', 'Launch options saved'), 'success')
    } catch (e) {
      notify(errorMessage(e), 'error')
    } finally {
      gsSaving = false
    }
  }

  // Move the game folder somewhere else (its own subfolder is kept)
  async function moveTo() {
    const g = game
    if (!g?.runner || !g.install) return
    const dir = await open({ directory: true, defaultPath: settings.value.defaultInstallPath || undefined, title: t('game.moveTo', 'Move to…') })
    if (typeof dir !== 'string') return
    const ok = await askConfirm({
      title: t('game.moveQ', 'Move the game?'),
      message: t('game.moveMessage', '{{title}} will be copied to {{path}} and removed from its current folder.', { title: g.title, path: dir }),
      confirmLabel: t('game.move', 'Move'),
      subject: g.title,
      cancelLabel: t('game.cancel', 'Cancel'),
      danger: false
    })
    if (!ok) return
    try {
      await moveGame(g.runner, g.appName, dir)
      notify(t('game.moved', '{{title}} moved', { title: g.title }), 'success')
    } catch (e) {
      notify(errorMessage(e), 'error', 9000)
    }
  }
  // what the preparation is doing (a download with its percentage), else the general hint
  const prepareTitle = $derived(
    status?.message
      ? `${status.message}${status.progress?.bytesTotal ? ` · ${Math.round(status.progress.percent)}%` : ''}`
      : t('game.preparingHint', "Setting up Proton, its runtime and the game's prefix. Downloads happen on the first start and after a Proton update.")
  )

  // Cloud saves of installed store games (the floppy menu next to Play);
  // hidden when the game has none.
  let saves = $state<CloudSaves | null>(null)
  async function loadSaves() {
    const g = game
    if (!g?.runner || !g.isInstalled || sideload) return
    try {
      saves = await getCloudSaves(g.runner, g.appName)
    } catch (e) {
      console.warn('cloud saves:', errorMessage(e))
    }
  }
  // a string, so library reloads that keep the game as it was do not refetch
  const savesKey = $derived(game?.isInstalled && !sideload ? `${game.runner}:${game.appName}` : '')
  $effect(() => {
    void savesKey
    saves = null
    untrack(() => void loadSaves())
  })
  // a sync around launch / exit may have created the save folder
  let lastStatus: string | undefined
  $effect(() => {
    const now = status?.status
    if (lastStatus === 'syncing' && now !== 'syncing') void loadSaves()
    lastStatus = now
  })

  async function changePath() {
    const dir = await open({ directory: true, defaultPath: installPath || undefined })
    if (typeof dir === 'string') installPath = dir
  }

  async function importExisting() {
    if (!game?.runner) return
    const dir = await open({ directory: true, defaultPath: installPath || undefined, title: t('game.importPick', 'Folder where {{title}} is installed', { title: game.title }) })
    if (typeof dir !== 'string') return
    importing = true
    try {
      await importGame(game.runner, game.appName, dir)
      notify(t('game.imported', '{{title}} imported', { title: game.title }), 'success')
    } catch (e) {
      notify(errorMessage(e), 'error', 9000)
    } finally {
      importing = false
    }
  }

  async function doUninstall() {
    if (!game) return
    const ok = await askConfirm(
      sideload
        ? {
            title: t('sideload.removeQ', 'Remove from the library?'),
            message: t('sideload.removeMessage', 'The game files stay on disk; only the card is removed.'),
            confirmLabel: t('sideload.remove', 'Remove from library'),
            subject: game.title,
            cancelLabel: t('game.cancel', 'Cancel'),
            danger: true
          }
        : {
            title: t('console.game.uninstallQ', 'Uninstall the game?'),
            message: game.thirdPartyManagedApp
              ? t('game.thirdPartyUninstall', 'Only the entry here is removed; uninstall the game itself in {{app}}.', { app: game.thirdPartyManagedApp })
              : host.linux ? t('console.game.uninstallMessageLinux', 'The game files and its Wine prefix (settings and local saves) will be deleted from this PC.') : t('console.game.uninstallMessage', 'The game files will be deleted from this PC.'),
            confirmLabel: t('game.uninstall', 'Uninstall'),
            subject: game.title,
            cancelLabel: t('game.cancel', 'Cancel'),
            danger: true
          }
    )
    if (ok && (await uninstall(game)) && sideload) back()
  }

  async function stop() {
    if (!game?.runner) return
    try {
      await killGame(game.runner, game.appName)
    } catch (e) {
      notify(errorMessage(e), 'error')
    }
  }
</script>

{#if game}
  <div class="page">
    <header class="hero">
      {#if hero}<img class="art" src={hero} alt="" />{/if}
      <div class="fade"></div>
      <button class="back" onclick={back} aria-label={t('console.back', 'Back')}>
        <svg viewBox="0 0 24 24"><path d="M15 5l-7 7 7 7" /></svg>
      </button>
      <div class="brand">
        {#if game.artLogo && !logoBroken}
          <img class="logo" src={game.artLogo} alt={game.title} onerror={() => (logoBroken = true)} />
        {:else}
          <h1>{game.title}</h1>
        {/if}
      </div>
    </header>

    <div class="bar">
      <div class="main-action">
        {#if downloading || moving}
          <div class="progress">
            <div class="progress-head">
              <strong>
                {moving ? t('game.moving', 'Moving…') : status?.status === 'queued' ? t('downloads.queued', 'Queued') : status?.status === 'paused' ? t('downloads.paused', 'Paused') : status?.status === 'updating' ? t('game.updating', 'Updating') : t('game.installing', 'Installing')}
                {#if status?.progress}<span class="pct">{status.progress.percent.toFixed(0)}%</span>{/if}
              </strong>
              {#if status?.progress}
                <span class="muted">{formatBytes(status.progress.bytesDone)} / {formatBytes(status.progress.bytesTotal)} · {formatSpeed(status.progress.speed)} · {formatEta(status.progress.eta)}</span>
              {/if}
            </div>
            <div class="track"><div class="fill" style:width={`${status?.progress?.percent ?? 0}%`}></div></div>
          </div>
          {#if status?.status === 'paused'}
            <button class="btn primary" onclick={() => game.runner && resume(game.runner, game.appName)}>{t('downloads.resume', 'Resume')}</button>
          {/if}
          {#if downloading}
            <button class="btn" onclick={() => game.runner && cancel(game.runner, game.appName)}>{status?.status === 'paused' ? t('downloads.remove', 'Remove') : t('game.cancel', 'Cancel')}</button>
          {/if}
        {:else if game.isInstalled}
          {#if playing}
            <button class="big stop" onclick={stop}>
              <svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="2" /></svg>{t('game.stop', 'Stop')}
            </button>
          {:else}
            <button class="big play accent-live" class:starting disabled={busy} title={status?.status === 'preparing' ? prepareTitle : undefined} onclick={() => launch(game)}>
              {#if !starting}<svg viewBox="0 0 24 24"><path d="M8 5.5v13l10.5-6.5z" /></svg>{/if}<MorphLabel text={playLabel} busy={starting} />
            </button>
          {/if}
          {#if saves?.supported}
            <CloudSavesMenu {game} {saves} onsynced={loadSaves} />
          {/if}
          {#if game.updateAvailable}
            <button class="big update" disabled={busy || playing} onclick={() => update(game)}>{t('game.update', 'Update')}</button>
          {/if}
          {#if status?.status === 'syncing'}
            <span class="calc"><span class="spinner"></span>{t('saves.syncing', 'Syncing saves…')}</span>
          {/if}
        {:else if !game.installable}
          <p class="muted">{t('game.thirdParty', 'This title is managed by {{app}} and cannot be installed here yet.', { app: game.thirdPartyManagedApp ?? '' })}</p>
        {:else}
          <button class="big install" disabled={busy || (!installPath && !game.thirdPartyManagedApp)} onclick={() => install(game, installPath, { branch: branch || undefined, buildId: buildId || undefined })}>
            <svg viewBox="0 0 24 24"><path d="M12 4v11M7 10.5l5 5 5-5M5 20h14" /></svg>{t('console.install', 'Install')}
          </button>
          {#if game.thirdPartyManagedApp}
            <p class="muted">{t('game.thirdPartyInstall', 'This title is managed by {{app}}. Install sets that app up on this PC; it downloads and starts the game.', { app: game.thirdPartyManagedApp })}</p>
          {/if}
        {/if}
      </div>

      <div class="stats">
        {#if game.isInstalled && game.thirdPartyManagedApp}
          <div class="stat"><span class="k">{t('game.managedBy', 'Managed by')}</span><span class="v">{game.thirdPartyManagedApp}</span></div>
        {:else if game.isInstalled && game.install && !sideload}
          <div class="stat"><span class="k">{t('game.size', 'Size')}</span><span class="v">{game.install.installSize}</span></div>
          <div class="stat">
            <span class="k">{t('game.version', 'Version')}</span>
            <span class="v">{game.install.version}{game.install.pinnedVersion ? ` · ${t('game.pinned', 'pinned')}` : ''}</span>
          </div>
          {#if game.install.branch}
            <div class="stat"><span class="k">{t('game.branch', 'Branch')}</span><span class="v">{game.install.branch}</span></div>
          {/if}
        {:else if game.installable && !downloading && !game.thirdPartyManagedApp}
          <div class="stat">
            <span class="k">{t('game.diskSize', 'On disk')}</span>
            <span class="v">
              {#if info}{formatBytes(info.diskSize)}
              {:else if infoError}<span class="err" title={infoError}>—</span>
              {:else}<span class="calc"><span class="spinner"></span>{t('game.calculating', 'Calculating size…')}</span>{/if}
            </span>
          </div>
          {#if info}
            <div class="stat"><span class="k">{t('game.downloadSize', 'Download')}</span><span class="v">{formatBytes(info.downloadSize)}</span></div>
          {/if}
          {#if info?.branches.length}
            <div class="stat">
              <span class="k">{t('game.branch', 'Branch')}</span>
              <select class="sel" bind:value={branch} onchange={() => (buildId = '')}>
                <option value="">{t('game.branchPublic', 'Public')}</option>
                {#each info.branches as b (b)}<option value={b}>{b}</option>{/each}
              </select>
            </div>
          {/if}
          {#if builds.length > 1}
            <div class="stat">
              <span class="k">{t('game.version', 'Version')}</span>
              <select class="sel" bind:value={buildId}>
                <option value="">{t('game.versionLatest', 'Latest')}</option>
                {#each builds as b (b.buildId)}
                  <option value={b.buildId}>{b.version || b.buildId}{b.date ? ` · ${new Date(b.date).toLocaleDateString(i18n.locale)}` : ''}</option>
                {/each}
              </select>
            </div>
          {/if}
          <div class="stat path">
            <span class="k">{t('game.installPath', 'Install to')}</span>
            <button class="v linkish" onclick={changePath} title={t('game.browse', 'Browse…')}>{installPath}</button>
          </div>
        {/if}
        <div class="stat"><span class="k">{t('game.store', 'Store')}</span><span class="v">{runnerName(game.runner)}</span></div>
      </div>

      <div class="tools">
        {#if game.isInstalled}
          <button class="icon" title={t('game.openFolder', 'Open folder')} onclick={openFolder}>
            <svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
          </button>
          {#if game.install?.installPath && !game.thirdPartyManagedApp}
            <button class="icon" title={t('game.moveTo', 'Move to…')} disabled={busy || playing} onclick={moveTo}>
              <svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v2M3 7v10a2 2 0 0 0 2 2h7M15 16h6m-3-3 3 3-3 3" /></svg>
            </button>
          {/if}
          {#if sideload}
            <button class="icon" title={t('sideload.edit', 'Edit')} disabled={busy || playing} onclick={() => (editing = true)}>
              <svg viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4" /></svg>
            </button>
          {:else}
            <button class="icon" title={t('downloads.repair', 'Verify files')} disabled={busy || playing} onclick={() => repair(game)}>
              <svg viewBox="0 0 24 24"><path d="M12 3l7 3v5c0 4.5-3 8.5-7 10-4-1.5-7-5.5-7-10V6zM9 12l2 2 4-4" /></svg>
            </button>
          {/if}
          <button class="icon danger" title={sideload ? t('sideload.remove', 'Remove from library') : t('game.uninstall', 'Uninstall')} disabled={busy || playing} onclick={doUninstall}>
            <svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3" /></svg>
          </button>
        {/if}
        {#if game.storeUrl}
          <button class="icon" title={t('game.storePage', 'Store page')} onclick={() => openExternal(game.storeUrl!).catch(report)}>
            <svg viewBox="0 0 24 24"><path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5" /></svg>
          </button>
        {/if}
      </div>
    </div>

    {#if !game.isInstalled && game.installable && !downloading}
      <p class="import-hint">
        {t('game.alreadyInstalled', 'Already installed on this PC?')}
        <button class="linkish" disabled={importing} onclick={importExisting}>
          {importing ? t('import.runningShort', 'Importing…') : t('game.importExisting', 'Import the existing folder')}
        </button>
      </p>
    {/if}

    <ScreenshotGallery shots={extra?.screenshots ?? []} loading={extraLoading && !sideload} />

    <div class="body">
      <div class="main">
      <section class="about">
        <h2>{t('game.about', 'About this game')}</h2>
        {#if description}
          {#each description.split(/\n{2,}/) as para, i (i)}<p class="desc">{para}</p>{/each}
        {:else if extraLoading}
          <p class="desc ph"></p>
          <p class="desc ph short"></p>
        {:else}
          <p class="muted">{t('game.noDescription', 'No description available.')}</p>
        {/if}
      </section>

      {#if dlcs.length}
        <section class="dlc">
          <h2>{t('dlc.title', 'Downloadable content')}</h2>
          <ul>
            {#each dlcs as d (d.appName)}
              {@const s = dlcStatus(d)}
              {@const working = !!s && s.status !== 'done' && s.status !== 'error'}
              <li>
                <span class="name">{d.title}</span>
                {#if working}
                  <span class="state">
                    {#if s.status === 'installing' && s.progress}{Math.floor(s.progress.percent)}%
                    {:else if s.status === 'uninstalling'}{t('game.uninstalling', 'Uninstalling…')}
                    {:else}{t('downloads.queued', 'Queued')}{/if}
                  </span>
                  {#if s.status === 'installing' || s.status === 'queued'}
                    <button class="small" onclick={() => game?.runner && cancel(game.runner, d.appName)}>{t('game.cancel', 'Cancel')}</button>
                  {/if}
                {:else if d.installed}
                  <span class="state installed">{t('dlc.installed', 'Installed')}</span>
                  {#if d.installable}
                    <button class="small" disabled={busy} onclick={() => uninstall(dlcGame(d))}>{t('game.uninstall', 'Uninstall')}</button>
                  {/if}
                {:else}
                  <button class="small accent" disabled={busy} onclick={() => install(dlcGame(d))}>{t('console.install', 'Install')}</button>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if achievements && achievements.total > 0}
        <section class="ach">
          <h2>
            {t('achievements.title', 'Achievements')}
            <span class="count">{t('achievements.unlocked', '{{done}} of {{total}} unlocked', { done: achievements.unlocked, total: achievements.total })} · {Math.round((achievements.unlocked / achievements.total) * 100)}%</span>
          </h2>
          <div class="ach-bar"><div class="fill" style:width={`${(achievements.unlocked / achievements.total) * 100}%`}></div></div>
          <ul class="ach-grid">
            {#each achievements.list as a (a.id)}
              <li class:unlocked={a.unlocked} class:hidden-ach={a.hidden} title={a.hidden ? '' : a.description}>
                <img src={a.unlocked ? a.iconUnlocked : a.iconLocked} alt="" loading="lazy" />
                <span class="ach-text">
                  <span class="ach-name">{a.hidden ? t('achievements.hidden', 'Hidden achievement') : a.name}</span>
                  <span class="ach-desc">{a.hidden ? '' : a.description}</span>
                  <span class="ach-meta">{a.unlocked ? achDate(a.unlockedAt) : a.rarityLabel}{a.rarity ? ` · ${a.rarity.toFixed(1)}%` : ''}</span>
                </span>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
      </div>

      <aside class="side">
        <section class="details">
        <h2>{t('game.details', 'Details')}</h2>
        <dl>
          {#if extra?.developer ?? game.developer}<dt>{t('game.developer', 'Developer')}</dt><dd>{extra?.developer ?? game.developer}</dd>{/if}
          {#if extra?.publisher}<dt>{t('console.game.publisher', 'Publisher')}</dt><dd>{extra.publisher}</dd>{/if}
          {#if releaseDate}<dt>{t('console.game.released', 'Release date')}</dt><dd>{releaseDate}</dd>{/if}
          <dt>{t('game.store', 'Store')}</dt><dd>{runnerName(game.runner) || '—'}</dd>
          {#if !sideload}
            <dt>{t('game.offline', 'Offline play')}</dt><dd>{game.canRunOffline ? t('common.yes', 'Yes') : t('common.no', 'No')}</dd>
            <dt>{t('game.cloudSaves', 'Cloud saves')}</dt><dd>{game.cloudSaveEnabled ? t('common.yes', 'Yes') : t('common.no', 'No')}</dd>
          {/if}
          {#if game.install}
            <dt>{t('game.path', 'Path')}</dt>
            <dd><button class="linkish mono" onclick={openFolder}>{game.install.installPath}</button></dd>
            <dt>{t('game.executable', 'Executable')}</dt><dd class="mono">{game.install.executable}</dd>
          {/if}
        </dl>
        {#if extra?.tags.length}
          <div class="tags">{#each extra.tags as tag (tag)}<span>{tag}</span>{/each}</div>
        {/if}
        </section>
        {#if gs && game.isInstalled}
          <details class="launch-options">
            <summary>{t('launchOptions.title', 'Launch options')}</summary>
            <label>
              <span>{t('launchOptions.args', 'Extra arguments')}</span>
              <input type="text" bind:value={gs.launchArgs} placeholder="-windowed -novid" spellcheck="false" />
            </label>
            <label>
              <span>{t('launchOptions.exe', 'Executable (instead of the default)')}</span>
              <input type="text" bind:value={gs.targetExe} placeholder="Bin\\Game.exe" spellcheck="false" />
            </label>
            <label>
              <span>{t('launchOptions.wrapper', 'Wrapper command')}</span>
              <input type="text" bind:value={gs.wrapper} placeholder={host.linux ? 'mangohud' : ''} spellcheck="false" />
            </label>
            <label>
              <span>{t('launchOptions.env', 'Environment variables (one KEY=value per line)')}</span>
              <textarea bind:value={gsEnv} rows="3" spellcheck="false"></textarea>
            </label>
            {#if game.runner === 'gog'}
              <label>
                <span>{t('launchOptions.gogPassword', 'GOG private branch password')}</span>
                <input type="password" bind:value={gs.gogBranchPassword} autocomplete="off" />
              </label>
            {/if}
            <button class="btn" disabled={gsSaving} onclick={saveLaunchOptions}>{t('launchOptions.save', 'Save')}</button>
          </details>
        {/if}
      </aside>
    </div>
  </div>
  {#if editing}
    <AddGameDialog {game} onclose={() => (editing = false)} />
  {/if}
{:else}
  <div class="missing">
    <button class="btn" onclick={back}>← {t('console.back', 'Back')}</button>
    <p class="muted">{t('game.notFound', 'Game not found')}</p>
  </div>
{/if}

<style>
  .page {
    min-height: 100%;
    padding-bottom: var(--space-xl);
  }
  .hero {
    position: relative;
    height: clamp(260px, 42vh, 520px);
    overflow: hidden;
    background: var(--background-darker);
  }
  .art {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: center 25%;
  }
  .fade {
    position: absolute;
    inset: 0;
    background:
      linear-gradient(to bottom, transparent 45%, color-mix(in srgb, var(--body-background) 85%, transparent) 88%, var(--body-background)),
      linear-gradient(to right, color-mix(in srgb, var(--body-background) 55%, transparent), transparent 55%);
  }
  .back {
    position: absolute;
    top: 16px;
    left: 16px;
    width: 38px;
    height: 38px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--body-background) 55%, transparent);
    backdrop-filter: blur(8px);
    color: var(--text-default);
  }
  .back:hover {
    background: color-mix(in srgb, var(--body-background) 80%, transparent);
  }
  .brand {
    position: absolute;
    left: var(--space-xl);
    bottom: var(--space-lg);
    right: 40%;
  }
  .logo {
    max-width: 100%;
    max-height: 140px;
    object-fit: contain;
    object-position: left bottom;
    filter: drop-shadow(0 4px 18px rgba(0, 0, 0, 0.6));
  }
  h1 {
    margin: 0;
    font-size: clamp(30px, 4vw, 52px);
    line-height: 1.05;
    color: #fff;
    text-shadow: 0 3px 20px rgba(0, 0, 0, 0.7);
  }

  /* action bar */
  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-lg);
    margin: 0 var(--space-xl);
    padding: 14px 18px;
    border-radius: 12px;
    background: var(--navbar-background);
    border: 1px solid var(--background-lighter);
  }
  .main-action {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
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
  .big {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    height: 46px;
    padding: 0 28px;
    border-radius: 8px;
    font-size: 17px;
    font-weight: 600;
    letter-spacing: 0.3px;
    transition: filter var(--transition), transform var(--transition);
  }
  .big:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .big:active:not(:disabled) {
    transform: scale(0.98);
  }
  .big:disabled {
    opacity: 0.5;
  }
  /* starting is progress, not "unavailable": keep the button lit */
  .big.play.starting:disabled {
    opacity: 1;
    cursor: progress;
  }
  /* the logo's look (gradient drift + ring): .accent-live in app.css */
  .big.play svg {
    width: 28px;
    height: 28px;
    /* pull the label back: the triangle has empty room on its left */
    margin-left: -6px;
  }
  .big.play svg,
  .big.stop svg {
    fill: currentColor;
    stroke: none;
  }
  .big.install {
    background: var(--accent);
    color: var(--accent-text);
  }
  .big.update {
    background: var(--background-lighter);
    color: var(--text-default);
    padding: 0 18px;
    font-size: 15px;
  }
  .big.stop {
    background: var(--danger);
    color: #fff;
  }
  .progress {
    width: min(420px, 40vw);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .progress-head {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 13px;
  }
  .pct {
    margin-left: 8px;
    color: var(--accent-overlay);
  }
  .track {
    height: 8px;
    border-radius: 4px;
    background: var(--background-lighter);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 400ms linear;
  }
  .stats {
    flex: 1;
    min-width: 0;
    display: flex;
    gap: var(--space-lg);
    overflow: hidden;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .stat.path {
    flex: 1;
  }
  .k {
    font-size: 11px;
    letter-spacing: 0.6px;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }
  .v {
    font-size: 14px;
    color: var(--text-default);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .calc {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    color: var(--text-secondary);
  }
  .spinner {
    width: 12px;
    height: 12px;
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
  .err {
    color: var(--danger);
  }
  .tools {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .icon {
    width: 40px;
    height: 40px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    color: var(--text-secondary);
    background: var(--background);
  }
  .icon:hover:not(:disabled) {
    color: var(--text-default);
    background: var(--current-background);
  }
  .icon.danger:hover:not(:disabled) {
    color: var(--danger);
  }
  .icon:disabled {
    opacity: 0.4;
  }
  .linkish {
    padding: 0;
    text-align: left;
    color: var(--text-default);
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 3px;
    transition: color var(--transition), text-decoration-color var(--transition);
  }
  .linkish:hover {
    color: var(--accent-overlay);
    text-decoration-color: currentColor;
  }
  .import-hint {
    margin: 10px var(--space-xl) 0;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .import-hint .linkish {
    color: var(--accent-overlay);
    margin-left: 4px;
  }

  /* content */
  .body {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(260px, 340px);
    gap: var(--space-lg);
    margin: var(--space-lg) var(--space-xl) 0;
    align-items: start;
  }
  h2 {
    margin: 0 0 10px;
    font-size: 13px;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--text-secondary);
  }
  .desc {
    margin: 0 0 12px;
    line-height: 1.6;
    color: var(--text-default);
    user-select: text;
  }
  .desc.ph {
    height: 4.8em;
    border-radius: 8px;
    background: var(--background);
    animation: pulse 1.1s ease-in-out infinite alternate;
  }
  .desc.ph.short {
    width: 60%;
    height: 3.2em;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: var(--space-md);
  }
  .tags span {
    padding: 3px 10px;
    border-radius: 999px;
    background: var(--background-lighter);
    font-size: 12px;
    color: var(--text-secondary);
  }
  .details {
    padding: var(--space-md) var(--space-lg);
    border-radius: 12px;
    background: var(--background);
  }
  /* one row, two stacks: the Details card must not stretch the About row */
  .main,
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--space-lg);
    min-width: 0;
  }
  .sel {
    padding: 2px 6px;
    border-radius: 6px;
    border: 1px solid var(--background-lighter);
    background: var(--background);
    color: var(--text-default);
    font: inherit;
    font-size: 13px;
    max-width: 260px;
  }
  .ach h2 {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
  }
  .ach .count {
    text-transform: none;
    letter-spacing: 0;
    font-size: 13px;
  }
  .ach-bar {
    height: 4px;
    margin-bottom: 12px;
    border-radius: 2px;
    background: var(--background-lighter);
    overflow: hidden;
  }
  .ach-bar .fill {
    height: 100%;
    background: var(--accent);
  }
  .ach-grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: 8px;
  }
  .ach-grid li {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 8px 10px;
    border-radius: 10px;
    background: var(--background);
    min-width: 0;
  }
  .ach-grid li:not(.unlocked) {
    opacity: 0.6;
  }
  .ach-grid li:not(.unlocked) img {
    filter: grayscale(1);
  }
  .ach-grid img {
    width: 48px;
    height: 48px;
    flex-shrink: 0;
    border-radius: 8px;
    object-fit: cover;
    background: var(--background-lighter);
  }
  .ach-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    font-size: 13px;
  }
  .ach-name {
    font-weight: 600;
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
    font-size: 12px;
  }
  .launch-options {
    padding: var(--space-md) var(--space-lg);
    border-radius: 12px;
    background: var(--background);
  }
  .launch-options summary {
    cursor: pointer;
    font-size: 13px;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--text-secondary);
  }
  .launch-options label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 12px;
    font-size: 13px;
    color: var(--text-secondary);
  }
  .launch-options input,
  .launch-options textarea {
    padding: 6px 10px;
    border-radius: 8px;
    border: 1px solid var(--background-lighter);
    background: var(--body-background);
    color: var(--text-default);
    font: inherit;
    font-size: 13px;
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
  }
  .launch-options .btn {
    margin-top: 12px;
  }
  .dlc ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .dlc li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 14px;
    border-radius: 10px;
    background: var(--background);
    font-size: 14px;
  }
  .dlc .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dlc .state {
    color: var(--text-secondary);
    font-size: 13px;
  }
  .dlc .state.installed {
    color: var(--accent);
  }
  .dlc .small {
    padding: 4px 12px;
    border-radius: 8px;
    font-size: 13px;
    background: var(--background-lighter);
    color: var(--text-default);
  }
  .dlc .small.accent {
    background: var(--accent);
    color: #fff;
  }
  .dlc .small:disabled {
    opacity: 0.5;
  }
  dl {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 8px 16px;
    margin: 0;
    font-size: 14px;
  }
  dt {
    color: var(--text-tertiary);
  }
  dd {
    margin: 0;
    min-width: 0;
    word-break: break-word;
  }
  .mono {
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    font-size: 12px;
    word-break: break-all;
  }
  .missing {
    padding: var(--space-lg) var(--space-xl);
  }
  @media (max-width: 1100px) {
    .body {
      grid-template-columns: 1fr;
    }
    .stat.path {
      display: none;
    }
  }
  @keyframes pulse {
    to {
      opacity: 0.5;
    }
  }
</style>
