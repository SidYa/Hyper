<script lang="ts">
  // Console mode: a 10-foot UI driven by gamepad or keyboard.
  // Port of HGL/src/frontend/screens/ConsoleMode/index.tsx.
  //
  // Input model: while mounted this screen pushes one gamepad handler.
  // Overlays (launch, install, confirm dialogs) and the on-screen keyboard
  // own input while they are open. Focus zones: the top bar (real DOM focus
  // on its buttons) and the tile grid (a focused index).
  import { tick } from 'svelte'
  import { on } from '$lib/api/events'
  import { cancel, gameStatus, install, launch, library, libraryStores, libraryTab, update, resume, isInstalledOrInstalling } from '$lib/state/library.svelte'
  import { settings } from '$lib/state/settings.svelte'
  import { host } from '$lib/state/host.svelte'
  import { navigate } from '$lib/router.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { errorMessage, getCloseBlockers, getInstallInfo, killGame, quitApp } from '$lib/api/commands'
  import { closeWarning, isBusy } from '$lib/state/close.svelte'
  import { notify } from '$lib/state/toast.svelte'
  import { formatBytes } from '$lib/format'
  import { gamepad, pushHandler, setActionListener, synthesize } from '$lib/gamepad/input.svelte'
  import { playSound, soundFor } from '$lib/sounds'
  import { directionOf, keyToAction, type Direction, type GamepadAction } from '$lib/gamepad/actions'
  import { getBackButtonIndex } from '$lib/gamepad/controller'
  import { keyboard, onKeyboardClosed, openKeyboard } from '$lib/state/keyboard.svelte'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'
  import ControllerHints from '$lib/components/ControllerHints.svelte'
  import Brand from '$lib/components/Brand.svelte'
  import { heroImage, prepareGames, runnerName } from '$lib/library/view'
  import { type GameInfo, type InstallInfo, type Runner, type CloseBlockers } from '$types/index'
  import ConsoleCard from './ConsoleCard.svelte'
  import ConsoleDialog from './ConsoleDialog.svelte'
  import ConsoleMenu, { type ConsolePage, type MenuItemId } from './ConsoleMenu.svelte'
  import ConsoleSettings from './ConsoleSettings.svelte'
  import ConsoleDownloads from './ConsoleDownloads.svelte'
  import ConsoleClock from './ConsoleClock.svelte'
  import ConsoleGame from './ConsoleGame.svelte'
  import Spinner from '$lib/components/Spinner.svelte'
  import { itemAt, items } from '$lib/dom'

  type StoreKey = Runner | 'all' | 'installed'
  const CANCEL_HOLD_MS = 2000

  // The tab asked for (the default from Settings, then the user's pick) is
  // kept apart from the one shown: while it is not available yet (stores
  // still loading at start) a fallback shows, and it comes back by itself.
  let wantedStore = $state<StoreKey>(settings.value.consoleModeDefaultTab ?? 'all')
  let searchOpen = $state(false)
  let searchText = $state('')
  let focusedIndex = $state(0)
  let zone = $state<'grid' | 'topbar'>('grid')

  let launching = $state<GameInfo | null>(null)
  let launchPending = $state(false)
  let installing = $state<GameInfo | null>(null)
  let installInfo = $state<InstallInfo | null>(null)
  let installError = $state<string | null>(null)
  let confirmCancel = $state<{ game: GameInfo; kind: 'install' | 'update' | 'repair' } | null>(null)
  // game page opened with Start on a tile
  let detail = $state<GameInfo | null>(null)
  let confirmQueued = $state<GameInfo | null>(null)
  let confirmResume = $state<GameInfo | null>(null)
  // what quitting now would interrupt (shown in the quit dialog)
  let quitBlockers = $state<CloseBlockers | null>(null)
  let confirmQuit = $state<GameInfo | null>(null)
  // set once the launched game reported a status, so the overlay only closes
  // when the game actually ends (not in the gap before the first event)
  let sawLaunchStatus = false
  // progress (0..1) of the in-game View+Start chord, pushed by Rust
  let chord = $state(0)
  let updateNotice = $state<GameInfo | null>(null)
  let notice = $state<string | null>(null)
  let holdStart = $state<number | null>(null)
  // Steam-style side menu and the console sub-pages it leads to
  let page = $state<ConsolePage>('library')
  let menuOpen = $state(false)
  let confirmQuitApp = $state(false)

  let topbarEl = $state<HTMLDivElement | undefined>()
  let searchInput = $state<HTMLInputElement | undefined>()
  let columns = $state(1)

  // a dialog or sheet of this screen owns input
  const overlay = $derived(
    !!launching || !!installing || !!confirmCancel || !!confirmQueued || !!confirmResume || !!updateNotice || !!confirmQuit || confirmQuitApp
  )
  const idle = $derived(page === 'library' && !menuOpen && !overlay)
  // the tile grid itself (not covered by the game page)
  const gridIdle = $derived(idle && !detail)
  // The background refresh at every start is quiet; the spinner shows only
  // while a store's library loads for the first time (right after logging
  // in), when its games and covers really take a while to appear.
  const firstLoad = $derived(library.refreshing.some((r) => !library.games.some((g) => g.runner === r)))

  const games = $derived.by(() => {
    // titles of launchers nobody supports stay hidden; EA / Ubisoft ones install through theirs
    let list = library.games.filter((g) => !g.thirdPartyManagedApp || g.installable)
    if (activeStore === 'installed') list = list.filter(isInstalledOrInstalling)
    else if (activeStore !== 'all') list = list.filter((g) => g.runner === activeStore)
    const q = searchText.trim().toLowerCase()
    if (q) list = list.filter((g) => g.title.toLowerCase().includes(q))
    return prepareGames(list, settings.value, true, isInstalledOrInstalling)
  })
  const focused = $derived(games[focusedIndex])

  // "All" only with two or more stores (libraryTab)
  const storeTabs = $derived(libraryStores())
  const showAll = $derived(storeTabs.length >= 2)
  const tabs = $derived<Array<{ key: StoreKey; label: string }>>([
    { key: 'installed', label: t('console.installed', 'Installed') },
    ...(showAll ? [{ key: 'all' as StoreKey, label: t('console.all', 'All') }] : []),
    ...storeTabs.map((r) => ({
      key: r as StoreKey,
      label: r === 'legendary' ? 'Epic' : runnerName(r)
    }))
  ])

  const activeStore = $derived<StoreKey>(libraryTab(wantedStore))
  $effect(() => {
    if (focusedIndex >= games.length) focusedIndex = Math.max(0, games.length - 1)
  })

  function computeColumns() {
    const cards = items(gridEl)
    if (cards.length < 2) return (columns = 1)
    const top = cards[0].offsetTop
    let n = 1
    for (let i = 1; i < cards.length && cards[i].offsetTop === top; i++) n++
    columns = Math.max(1, n)
  }
  $effect(() => {
    void games.length
    void page
    tick().then(computeColumns)
    window.addEventListener('resize', computeColumns)
    return () => window.removeEventListener('resize', computeColumns)
  })

  // Programmatic scrolling slides the grid under a stationary mouse and the
  // browser then fires mouseenter on whatever card ends up below it. Hover
  // focus therefore follows the mouse only once it really moved after the
  // last keyboard / controller step (and never mid-scroll).
  let scrolling = false
  let scrollTimer: number | undefined
  let pointerMoved = false
  function onPointerMove(e: PointerEvent) {
    if (e.movementX || e.movementY) pointerMoved = true
  }
  // Scroll only as much as needed to keep the focused row visible, and skip
  // the smooth animation while a direction is held (key-repeat), otherwise
  // the animations queue up and the grid lags behind the focus.
  let gridEl = $state<HTMLDivElement | undefined>()
  let lastMoveAt = 0
  let prevMoveAt = 0
  const MARGIN = 72
  function revealFocused() {
    const el = itemAt(gridEl, focusedIndex)
    const box = gridEl
    if (!el || !box) return
    const top = el.offsetTop
    const bottom = top + el.offsetHeight
    const viewTop = box.scrollTop + MARGIN
    const viewBottom = box.scrollTop + box.clientHeight - MARGIN
    const maxScroll = box.scrollHeight - box.clientHeight
    const row = Math.floor(focusedIndex / columns)
    const lastRow = Math.floor((games.length - 1) / columns)
    let target: number | null = null
    // first / last row: go all the way, so the enlarged focused card and its
    // outline never end up clipped at the edge (above the footer)
    if (row === 0) target = box.scrollTop > 0 ? 0 : null
    else if (row === lastRow) target = box.scrollTop < maxScroll - 1 ? maxScroll : null
    else if (top < viewTop) target = top - MARGIN
    else if (bottom > viewBottom) target = bottom - box.clientHeight + MARGIN
    if (target === null) return
    scrolling = true
    window.clearTimeout(scrollTimer)
    scrollTimer = window.setTimeout(() => (scrolling = false), 700)
    const rapid = lastMoveAt - prevMoveAt < 250
    const smooth = !rapid
    box.scrollTo({ top: Math.min(maxScroll, Math.max(0, target)), behavior: smooth ? 'smooth' : 'auto' })
  }
  $effect(() => {
    void focusedIndex
    void games.length
    void page
    tick().then(revealFocused)
  })

  let noticeTimer: number | undefined
  function toast(msg: string) {
    notice = msg
    window.clearTimeout(noticeTimer)
    noticeTimer = window.setTimeout(() => (notice = null), 2600)
  }

  // --- grid / top bar movement ---
  function moveGrid(dir: Direction) {
    if (games.length === 0) return
    pointerMoved = false
    prevMoveAt = lastMoveAt
    lastMoveAt = performance.now()
    const last = games.length - 1
    if (dir === 'left') focusedIndex = Math.max(0, focusedIndex - 1)
    else if (dir === 'right') focusedIndex = Math.min(last, focusedIndex + 1)
    else if (dir === 'up') focusedIndex = Math.max(0, focusedIndex - columns)
    else if (dir === 'down') focusedIndex = Math.min(last, focusedIndex + columns)
  }
  function topbarButtons(): HTMLButtonElement[] {
    return Array.from(topbarEl?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])
  }
  function stepTopbar(delta: 1 | -1) {
    const btns = topbarButtons()
    if (!btns.length) return
    const active = document.activeElement as HTMLButtonElement | null
    const idx = active ? btns.indexOf(active) : -1
    let next: number
    if (idx !== -1) next = (idx + delta + btns.length) % btns.length
    else {
      const activeTab = btns.findIndex((b) => b.classList.contains('on'))
      const base = activeTab !== -1 ? activeTab : delta === 1 ? -1 : 0
      next = (base + delta + btns.length) % btns.length
    }
    zone = 'topbar'
    btns[next].focus()
  }
  function leaveTopbar() {
    zone = 'grid'
    ;(document.activeElement as HTMLElement | null)?.blur()
  }

  // --- actions ---
  function selectTab(key: StoreKey) {
    if (activeStore === key && wantedStore === key) return
    wantedStore = key
    focusedIndex = 0
  }
  // X (or /) searches the tab you are on; the top-bar search button searches
  // every store, so reaching it switches to "All".
  function searchAll() {
    if (showAll) selectTab('all')
  }
  function openSearch() {
    if (!searchInput) return
    searchOpen = true
    openKeyboard(searchInput, settings.value.consoleModeKeyboardLayout)
  }
  function onSearchInput() {
    searchText = searchInput?.value ?? ''
    focusedIndex = 0
  }
  $effect(() => {
    onKeyboardClosed(() => {
      searchOpen = false
      zone = 'grid'
    })
    return () => onKeyboardClosed(null)
  })

  // The launch overlay stays up while the game runs (like Heroic / Big
  // Picture) and closes once the game is gone or the launch failed.
  $effect(() => {
    if (!launching || launchPending) return
    const s = gameStatus(launching)
    if (s) sawLaunchStatus = true
    else if (sawLaunchStatus) {
      launching = null
      window.clearTimeout(holdTimer)
      holdStart = null
      holdBy = null
      sawLaunchStatus = false
    }
  })

  // Rust reports the View+Start chord while a game has focus.
  $effect(() => {
    let off: (() => void) | undefined
    let gone = false
    on('gamepad:quit-combo', (p) => (chord = p)).then((u) => {
      // unmounted before the listener was registered: drop it at once
      if (gone) u()
      else off = u
    })
    return () => {
      gone = true
      off?.()
    }
  })

  // A / Enter / a click on a tile opens the game page, like Steam Big
  // Picture; playing, installing or cancelling happens there.
  function openDetail(game = focused) {
    if (!game || !idle) return
    detail = game
  }

  function doResume() {
    const g = confirmResume
    confirmResume = null
    if (g?.runner) void resume(g.runner, g.appName)
  }

  // `fromDetail`: the game page already shows the sizes and the folder, so
  // Install there queues the download at once instead of opening the sheet
  async function activate(game = focused, fromDetail = false) {
    if (!game || !idle) return
    const s = gameStatus(game)
    if (s?.status === 'queued') return (confirmQueued = game)
    if (s?.status === 'paused') return (confirmResume = game)
    if (s?.status === 'installing') return (confirmCancel = { game, kind: 'install' })
    if (s?.status === 'updating') return (confirmCancel = { game, kind: 'update' })
    if (s?.status === 'repairing') return (confirmCancel = { game, kind: 'repair' })
    if (s?.status === 'playing') return (confirmQuit = game)
    if (!game.isInstalled) {
      if (!game.installable) {
        return toast(t('game.thirdParty', 'This title is managed by {{app}} and cannot be installed here yet.', { app: game.thirdPartyManagedApp ?? '' }))
      }
      if (fromDetail && !game.thirdPartyManagedApp) {
        await install(game, settings.value.defaultInstallPath)
        toast(t('console.queued', '{{title}} added to downloads', { title: game.title }))
        return
      }
      installing = game
      installInfo = null
      installError = null
      if (game.runner) {
        // the answer is for this sheet only: dropped once it is closed or shows another game
        const current = () => installing?.runner === game.runner && installing?.appName === game.appName
        getInstallInfo(game.runner, game.appName)
          .then((i) => {
            if (current()) installInfo = i
          })
          .catch((e) => {
            if (current()) installError = errorMessage(e)
          })
      }
      return
    }
    if (game.updateAvailable) return (updateNotice = game)
    await startLaunch(game)
  }

  async function startLaunch(game: GameInfo) {
    launching = game
    sawLaunchStatus = false
    launchPending = true
    const ok = await launch(game)
    launchPending = false
    // a failed or cancelled launch already says so in a toast / the overlay
    if (ok && settings.value.consoleModeSounds) playSound('launch')
    if (!ok) launching = null
  }

  // Interface sounds for controller / keyboard navigation (setting).
  $effect(() => {
    if (!settings.value.consoleModeSounds) return
    return setActionListener((action) => {
      const sound = soundFor(action)
      if (sound) playSound(sound)
    })
  })

  async function confirmInstall() {
    if (!installing) return
    const g = installing
    installing = null
    await install(g, settings.value.defaultInstallPath)
    toast(t('console.queued', '{{title}} added to downloads', { title: g.title }))
  }

  // Hold B / Escape for 2 s while launching to kill the game. Each source
  // ends only its own hold: the pad watcher below would otherwise cancel an
  // Escape hold at once, since no B is down.
  let holdTimer: number | undefined
  let holdBy: 'pad' | 'key' | null = null
  function startHold(by: 'pad' | 'key') {
    if (!launching || holdStart !== null) return
    holdStart = Date.now()
    holdBy = by
    holdTimer = window.setTimeout(async () => {
      const g = launching
      launching = null
      holdStart = null
      holdBy = null
      if (g?.runner) {
        try {
          await killGame(g.runner, g.appName)
        } catch {
          /* already gone */
        }
      }
    }, CANCEL_HOLD_MS)
  }
  function stopHold(by: 'pad' | 'key') {
    if (holdBy !== by) return
    window.clearTimeout(holdTimer)
    holdStart = null
    holdBy = null
  }

  async function quitGame(g: GameInfo) {
    confirmQuit = null
    if (!g.runner) return
    try {
      await killGame(g.runner, g.appName)
    } catch {
      /* already closed */
    }
  }

  function exit() {
    navigate('/library')
  }

  function openMenu() {
    if (zone === 'topbar') leaveTopbar()
    menuOpen = true
  }
  // force: the quit dialog already warned about running downloads / syncs
  function quitNow() {
    quitApp(true).catch((e) => notify(errorMessage(e), 'error'))
  }
  function onMenuSelect(id: MenuItemId) {
    menuOpen = false
    if (id === 'exitConsole') exit()
    else if (id === 'quitApp') {
      page = 'library'
      quitBlockers = null
      if (host.alwaysConsole) {
        // Deck edition: quit at once; ask only while a download or a save
        // sync runs, as cutting a sync short could lose saves
        getCloseBlockers()
          .then((b) => {
            if (isBusy(b)) {
              quitBlockers = b
              confirmQuitApp = true
            } else quitNow()
          })
          .catch(() => quitNow())
        return
      }
      confirmQuitApp = true
      getCloseBlockers()
        .then((b) => (quitBlockers = b))
        .catch(() => {})
    } else page = id
  }

  // --- gamepad ---
  $effect(() => {
    const off = pushHandler((action: GamepadAction): boolean => {
      if (keyboard.open) return false // keyboard handler sits above us
      const dir = directionOf(action)

      // Overlays
      if (launching) {
        if (action === 'back') startHold('pad')
        return true
      }
      if (installing) {
        if (action === 'mainAction') void confirmInstall()
        else if (action === 'back') installing = null
        return true
      }
      if (confirmCancel) {
        if (action === 'mainAction') {
          const { game } = confirmCancel
          confirmCancel = null
          if (game.runner) void cancel(game.runner, game.appName)
        } else if (action === 'back') confirmCancel = null
        return true
      }
      if (confirmQuit) {
        if (action === 'mainAction') void quitGame(confirmQuit)
        else if (action === 'back') confirmQuit = null
        return true
      }
      if (confirmQueued) {
        if (action === 'mainAction') {
          const g = confirmQueued
          confirmQueued = null
          if (g.runner) void cancel(g.runner, g.appName)
        } else if (action === 'back') confirmQueued = null
        return true
      }
      if (confirmResume) {
        if (action === 'mainAction') doResume()
        else if (action === 'back') confirmResume = null
        return true
      }
      if (updateNotice) {
        const g = updateNotice
        if (action === 'mainAction') {
          updateNotice = null
          void update(g)
        } else if (action === 'altAction') {
          updateNotice = null
          void startLaunch(g)
        } else if (action === 'back') updateNotice = null
        return true
      }
      if (confirmQuitApp) {
        if (action === 'mainAction') quitNow()
        else if (action === 'back') confirmQuitApp = false
        return true
      }
      // the game page sits above and takes input while no overlay is up
      if (detail) return true

      // Idle
      if (dir) {
        if (zone === 'topbar') {
          if (dir === 'down') leaveTopbar()
          else if (dir === 'left') stepTopbar(-1)
          else if (dir === 'right') stepTopbar(1)
        } else moveGrid(dir)
        return true
      }
      switch (action) {
        case 'mainAction':
          if (zone === 'topbar') (document.activeElement as HTMLElement | null)?.click()
          else openDetail()
          return true
        case 'back':
        case 'guide':
          openMenu()
          return true
        case 'start':
          if (focused && zone === 'grid') detail = focused
          return true
        case 'prevPage':
          stepTopbar(-1)
          return true
        case 'nextPage':
          stepTopbar(1)
          return true
        case 'rightClick':
          openSearch()
          return true
        default:
          return true
      }
    })
    return off
  })

  // Release a held B when the button comes up: the engine only reports
  // presses, so watch the raw pad while the launch overlay is up.
  $effect(() => {
    if (!launching) return
    let raf = 0
    const backIndex = getBackButtonIndex(gamepad.layout)
    const tick = () => {
      const anyBack = Array.from(navigator.getGamepads()).some((gp) => gp?.buttons[backIndex]?.pressed)
      if (!anyBack && holdStart !== null) stopHold('pad')
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  })

  // --- keyboard (desk use and testing) ---
  function onKey(e: KeyboardEvent) {
    if (keyboard.open) return
    // A held key repeats only for the arrows (moving along); a held Escape
    // (the launch cancel) or Enter fires once, like a controller button, or
    // the "back" sound rattles for as long as it is down
    if (e.repeat && !e.key.startsWith('Arrow')) return
    // The menu, the sub-pages and the quit dialog take keys as gamepad actions
    if (menuOpen || page !== 'library' || confirmQuitApp || (detail && !overlay)) {
      const action = keyToAction(e)
      if (action) {
        e.preventDefault()
        synthesize(action)
      }
      return
    }
    // The grid reads keys directly instead of through the action dispatcher
    // (which plays the sounds for the pages above), so it sounds them here.
    if (settings.value.consoleModeSounds) {
      const action = keyToAction(e)
      const sound = action && soundFor(action)
      if (sound) playSound(sound)
    }
    const map: Record<string, Direction> = { ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right' }
    if (launching) {
      if (e.key === 'Escape' && !e.repeat) startHold('key')
      return
    }
    if (!idle) {
      if (e.key === 'Escape') {
        installing = null
        confirmCancel = null
        confirmQueued = null
        confirmResume = null
        updateNotice = null
      } else if (e.key === 'Enter') {
        if (installing) void confirmInstall()
        else if (confirmResume) doResume()
      }
      return
    }
    if (map[e.key]) {
      e.preventDefault()
      if (zone === 'topbar') {
        if (map[e.key] === 'down') leaveTopbar()
        else if (map[e.key] === 'left') stepTopbar(-1)
        else if (map[e.key] === 'right') stepTopbar(1)
        else if (map[e.key] === 'up') {
          /* stay */
        }
      } else if (map[e.key] === 'up' && focusedIndex < columns) stepTopbar(1)
      else moveGrid(map[e.key])
    } else if (e.key === 'Enter') {
      if (zone === 'grid') openDetail()
    } else if (e.key === 'Escape') openMenu()
    else if ((e.key === 'i' || e.key === 'ContextMenu') && zone === 'grid' && focused) {
      e.preventDefault()
      detail = focused
    }
    else if (e.key === 'PageUp') stepTopbar(-1)
    else if (e.key === 'PageDown') stepTopbar(1)
    else if (e.key === '/' || e.code === 'Slash') {
      e.preventDefault()
      openSearch()
    }
  }
  function onKeyUp(e: KeyboardEvent) {
    if (e.key === 'Escape') stopHold('key')
  }

  const hints = $derived([
    { buttons: ['main' as const], label: t('console.details', 'Details') },
    { buttons: ['back' as const], label: t('console.menu.title', 'Menu') },
    { buttons: ['l1' as const, 'r1' as const], label: t('console.topbar', 'Top bar') },
    { buttons: ['x' as const], label: t('console.search', 'Search') },
    { buttons: ['dpad' as const, 'stick' as const], label: t('console.navigate', 'Navigate') }
  ])
</script>

<svelte:window onkeydown={onKey} onkeyup={onKeyUp} onpointermove={onPointerMove} />

<div class="console" class:paged={page !== 'library'} class:hide-cursor={gamepad.usingGamepad} data-theme={settings.value.consoleModeTheme}>
  {#if page === 'library'}
  <div class="topbar" bind:this={topbarEl}>
    <div class="brand"><Brand size={26} /></div>
    <!-- the focused game's title, in the free space of the top bar; not needed
         while every tile carries its title -->
    <div class="focused-title">{settings.value.consoleModeShowCardTitles ? '' : (focused?.title ?? '')}</div>
    <div class="tabs">
      {#each tabs as tab (tab.key)}
        <button class="chip" class:on={activeStore === tab.key} onclick={() => selectTab(tab.key)} onfocus={() => {
            // store tabs switch as soon as they get focus (like Steam); the
            // other top-bar buttons still need A
            zone = 'topbar'
            selectTab(tab.key)
          }} disabled={!gridIdle}>
          {tab.label}
        </button>
        {#if tab.key === 'installed'}<span class="divider"></span>{/if}
      {/each}
    </div>
    <div class="right">
      <button class="chip icon" class:on={searchOpen || !!searchText} onclick={() => {
          searchAll()
          openSearch()
        }} onfocus={() => {
          zone = 'topbar'
          searchAll()
        }} aria-label={t('console.search', 'Search')} disabled={!gridIdle}>
        <svg class="search-ico" viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5" /><path d="M16 16l4.5 4.5" /></svg>
      </button>
      <button class="chip icon" onclick={openMenu} onfocus={() => (zone = 'topbar')} aria-label={t('console.menu.title', 'Menu')} disabled={!gridIdle}>
        <svg class="search-ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M4 12h16M4 17h16" /></svg>
      </button>
      {#if firstLoad}
        <span class="refreshing" title={t('console.refreshing', 'Updating the library…')}><Spinner size={22} /></span>
      {/if}
      <span class="clock"><ConsoleClock /></span>
    </div>
    <!-- Hidden input the on-screen keyboard types into -->
    <input class="search-input" bind:this={searchInput} oninput={onSearchInput} tabindex="-1" aria-hidden="true" />
  </div>

  {#if games.length === 0}
    <div class="empty">
      {#if library.loading || library.refreshing.length}
        <span class="loading"><Spinner size={44} />{t('console.loading', 'Loading your library…')}</span>
      {:else if searchText.trim()}
        {t('console.searchEmpty', 'No games match "{{query}}".', { query: searchText.trim() })}
      {:else}
        {t('console.empty', 'No games to show')}
      {/if}
    </div>
  {:else}
    <div
      class="grid"
      role="list"
      class:dim={zone === 'topbar'}
      bind:this={gridEl}
      oncontextmenu={(e) => {
        e.preventDefault()
        if (focused && gridIdle) detail = focused
      }}
    >
      {#each games as game, i (`${game.runner}:${game.appName}`)}
        <ConsoleCard
          {game}
          status={gameStatus(game)}
          showTitle={settings.value.consoleModeShowCardTitles}
          showStore={settings.value.consoleModeShowCardStore}
          showPercent={settings.value.consoleShowProgressPercent}
          focused={zone === 'grid' && i === focusedIndex}
          onfocus={() => {
            if (pointerMoved && !scrolling) {
              focusedIndex = i
              zone = 'grid'
            }
          }}
          onselect={() => {
            focusedIndex = i
            zone = 'grid'
            openDetail(game)
          }}
        />
      {/each}
    </div>
  {/if}

  <div class="footer">
    {#if gamepad.connected && gridIdle && !keyboard.open}
      <ControllerHints layout={gamepad.layout} {hints} />
    {:else if !gamepad.connected}
      <span class="muted">{t('console.noGamepad', 'No controller detected. Use keyboard arrows, Enter and Escape.')}</span>
    {/if}
  </div>
    {#if detail}
      <ConsoleGame game={detail} active={!overlay && !menuOpen} onplay={(g) => void activate(g, true)} onback={() => (detail = null)} />
    {/if}
  {:else if page === 'downloads'}
    <ConsoleDownloads onback={() => (page = 'library')} />
  {:else}
    <ConsoleSettings onback={() => (page = 'library')} />
  {/if}

  {#if launching}
    {@const isPlaying = gameStatus(launching)?.status === 'playing'}
    <div class="overlay" data-focus-scope>
      <div class="overlay-card">
        <div class="spinner" class:idle={isPlaying}></div>
        <h2>
          {isPlaying
            ? t('console.playing', 'Playing {{title}}', { title: launching.title })
            : gameStatus(launching)?.status === 'syncing'
              ? t('console.syncing', 'Syncing saves of {{title}}…', { title: launching.title })
              : gameStatus(launching)?.status === 'preparing'
                ? t('console.preparing', 'Preparing {{title}}…', { title: launching.title })
                : t('console.launching', 'Launching {{title}}…', { title: launching.title })}
        </h2>
        {#if gameStatus(launching)?.status === 'preparing'}
          {@const prep = gameStatus(launching)}
          <p class="muted small">
            {#if prep?.message}
              {prep.message}{#if prep.progress?.bytesTotal}&nbsp;· {Math.round(prep.progress.percent)}%{/if}
            {:else}
              {t('game.preparingHint', "Setting up Proton, its runtime and the game's prefix. Downloads happen on the first start and after a Proton update.")}
            {/if}
          </p>
        {/if}
        <p class="muted hold" class:active={holdStart !== null}>
          <span class="hold-line">
            {t('console.holdPrefix', 'Hold')}
            {#if gamepad.connected}<ButtonGlyph button="back" layout={gamepad.layout} />{:else}<kbd>Esc</kbd>{/if}
            {isPlaying ? t('console.holdQuit', 'for 2 s to close the game') : t('console.holdSuffix', 'for 2 s to cancel')}
          </span>
          {#if holdStart !== null}<span class="hold-bar"><span class="hold-fill"></span></span>{/if}
        </p>
        {#if isPlaying}
          <p class="muted small">{t('console.chordHint', 'Inside the game: hold View + Start for 2 s')}</p>
        {/if}
      </div>
    </div>
  {:else if installing}
    <!-- Big Picture style install sheet: key art behind, logo/title, sizes, A to install -->
    <div class="sheet" data-focus-scope>
      {#if heroImage(installing)}<img class="sheet-art" src={heroImage(installing)} alt="" />{/if}
      <div class="sheet-fade"></div>
      <div class="sheet-body">
        {#if installing.artLogo}
          <img class="sheet-logo" src={installing.artLogo} alt={installing.title} />
        {:else}
          <h2 class="sheet-title">{installing.title}</h2>
        {/if}
        {#if installing.thirdPartyManagedApp}
          <p class="sheet-note">{t('game.thirdPartyInstall', 'This title is managed by {{app}}. Install sets that app up on this PC; it downloads and starts the game.', { app: installing.thirdPartyManagedApp })}</p>
        {:else}
          <div class="sheet-stats">
            <div class="st">
              <span class="k">{t('game.diskSize', 'On disk')}</span>
              {#if installInfo}<span class="v">{formatBytes(installInfo.diskSize)}</span>
              {:else if installError}<span class="v err">{installError}</span>
              {:else}<span class="v calc"><span class="spin"></span>{t('game.calculating', 'Calculating size…')}</span>{/if}
            </div>
            {#if installInfo}
              <div class="st"><span class="k">{t('game.downloadSize', 'Download')}</span><span class="v">{formatBytes(installInfo.downloadSize)}</span></div>
            {/if}
            <div class="st"><span class="k">{t('game.installPath', 'Install to')}</span><span class="v small">{settings.value.defaultInstallPath}</span></div>
          </div>
        {/if}
        <div class="sheet-actions">
          <button class="big-install" onclick={confirmInstall} data-focus-initial>
            <ButtonGlyph button="main" layout={gamepad.layout} />{t('console.install', 'Install')}
          </button>
          <button class="big-back" onclick={() => (installing = null)} data-dismiss>
            <ButtonGlyph button="back" layout={gamepad.layout} />{t('console.back', 'Back')}
          </button>
        </div>
      </div>
    </div>
  {:else if confirmCancel}
    <ConsoleDialog
      title={confirmCancel.kind === 'update'
        ? t('console.cancelUpdate.title', 'Cancel update?')
        : confirmCancel.kind === 'repair'
          ? t('console.cancelRepair.title', 'Stop verifying?')
          : t('console.cancelInstall.title', 'Cancel installation?')}
      message={confirmCancel.kind === 'update'
        ? t('console.cancelUpdate.message', 'This game is currently downloading. Cancel the ongoing update?')
        : confirmCancel.kind === 'repair'
          ? t('console.cancelRepair.message', 'The game files are being checked and repaired. Stop now?')
          : t('console.cancelInstall.message', 'This game is currently installing. Cancel the installation?')}
      gameTitle={confirmCancel.game.title}
      confirmLabel={t('console.cancelDownload', 'Cancel download')}
      cancelLabel={t('console.keep', 'Keep')}
      danger
      onconfirm={() => {
        const { game } = confirmCancel!
        confirmCancel = null
        if (game.runner) void cancel(game.runner, game.appName)
      }}
      oncancel={() => (confirmCancel = null)}
    />
  {:else if confirmQuit}
    <ConsoleDialog
      title={t('console.quitGame.title', 'Close the game?')}
      message={t('console.quitGame.message', 'Unsaved progress may be lost.')}
      gameTitle={confirmQuit.title}
      confirmLabel={t('console.quitGame.confirm', 'Close game')}
      cancelLabel={t('console.keep', 'Keep')}
      danger
      onconfirm={() => quitGame(confirmQuit!)}
      oncancel={() => (confirmQuit = null)}
    />
  {:else if confirmQueued}
    <ConsoleDialog
      title={t('console.removeFromQueue.title', 'Remove from queue?')}
      message={t('console.removeFromQueue.message', 'This game is queued for download. Remove it from the queue?')}
      gameTitle={confirmQueued.title}
      confirmLabel={t('console.remove', 'Remove')}
      cancelLabel={t('console.keep', 'Keep')}
      danger
      onconfirm={() => {
        const g = confirmQueued!
        confirmQueued = null
        if (g.runner) void cancel(g.runner, g.appName)
      }}
      oncancel={() => (confirmQueued = null)}
    />
  {:else if updateNotice}
    <ConsoleDialog
      title={t('console.update.title', 'Update available')}
      message={t('console.update.message', 'Update now, or play the installed version?')}
      gameTitle={updateNotice.title}
      confirmLabel={t('game.update', 'Update')}
      cancelLabel={t('console.playAnyway', 'Play anyway')}
      onconfirm={() => {
        const g = updateNotice!
        updateNotice = null
        void update(g)
      }}
      oncancel={() => {
        const g = updateNotice!
        updateNotice = null
        void startLaunch(g)
      }}
    />
  {:else if confirmResume}
    <ConsoleDialog
      title={t('console.resumeDownloadQ.title', 'Resume download?')}
      message={t('console.resumeDownloadQ.message', 'This download was left from the last session and continues where it stopped.')}
      gameTitle={confirmResume.title}
      confirmLabel={t('downloads.resume', 'Resume')}
      cancelLabel={t('console.keep', 'Keep')}
      onconfirm={doResume}
      oncancel={() => (confirmResume = null)}
    />
  {:else if confirmQuitApp}
    <ConsoleDialog
      title={t('console.quitAppQ.title', 'Quit Hyper?')}
      message={isBusy(quitBlockers) ? closeWarning(quitBlockers) : undefined}
      confirmLabel={isBusy(quitBlockers) ? t('close.quit', 'Quit anyway') : t('console.quitAppQ.confirm', 'Quit')}
      cancelLabel={t('console.keep', 'Keep')}
      danger
      onconfirm={quitNow}
      oncancel={() => (confirmQuitApp = false)}
    />
  {/if}
  {#if chord > 0}
    <div class="chord" style:--c={chord}>
      <span class="chord-ring"></span>{t('console.chordClosing', 'Closing the game…')}
    </div>
  {/if}
  {#if notice}
    <div class="toast">{notice}</div>
  {/if}
  {#if menuOpen}
    <ConsoleMenu current={page} onselect={onMenuSelect} onclose={() => (menuOpen = false)} />
  {/if}
</div>

<style>
  .console {
    --console-focus: #ffffff;
    /* the focused game tile wears the theme's accent (white: --console-focus) */
    --console-card-focus: var(--accent-overlay);
    position: relative;
    height: 100vh;
    display: grid;
    grid-template-rows: auto 1fr auto;
    background: radial-gradient(1200px 600px at 50% -10%, color-mix(in srgb, var(--accent) 24%, var(--body-background)) 0%, var(--body-background) 60%);
    color: var(--text-default);
    overflow: hidden;
  }
  .console.paged {
    grid-template-rows: 1fr;
  }
  .console.hide-cursor,
  .console.hide-cursor * {
    cursor: none;
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 24px;
    padding: 28px 56px 8px;
  }
  .brand {
    display: flex;
  }
  .tabs,
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .focused-title {
    flex: 1;
    min-width: 0;
    margin: 0 16px 0 4px;
    text-align: left;
    font-size: 22px;
    font-weight: 600;
    color: var(--text-default);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .divider {
    width: 1px;
    height: 22px;
    background: rgba(255, 255, 255, 0.18);
    margin: 0 6px;
  }
  .chip {
    padding: 10px 18px;
    border-radius: 999px;
    font-size: 17px;
    color: var(--text-secondary);
    outline: none;
    transition: background 140ms ease, color 140ms ease, box-shadow 140ms ease;
  }
  .chip.on {
    background: rgba(255, 255, 255, 0.14);
    color: var(--text-default);
  }
  .chip:focus {
    box-shadow: 0 0 0 3px var(--console-focus);
    color: var(--text-default);
  }
  .chip:disabled {
    opacity: 0.4;
  }
  .clock {
    margin-left: 14px;
  }
  .refreshing {
    display: flex;
    margin-left: 10px;
  }
  .loading {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 18px;
  }
  .search-input {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
  .search-ico {
    width: 20px;
    height: 20px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2.2;
    stroke-linecap: round;
    vertical-align: -4px;
  }
  .grid {
    position: relative;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 36px 28px;
    align-content: start;
    align-items: start;
    /* top: room for the enlarged first-row card, its outline and glow
       (scale 1.06 + 7px ring + the shadow), which overflow-y clips */
    padding: 40px 56px 64px;
    overflow-y: auto;
    scrollbar-width: none;
    transition: opacity 160ms ease;
  }
  .grid.dim {
    opacity: 0.7;
  }
  .grid::-webkit-scrollbar {
    display: none;
  }
  .empty {
    display: grid;
    place-items: center;
    font-size: 22px;
    color: var(--text-secondary);
  }
  .footer {
    display: flex;
    justify-content: center;
    align-items: center;
    min-height: 56px;
    padding: 10px 56px 22px;
    background: linear-gradient(to top, rgba(0, 0, 0, 0.5), transparent);
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(5, 6, 15, 0.82);
    backdrop-filter: blur(8px);
    z-index: 20;
  }
  .overlay-card {
    text-align: center;
  }
  .sheet {
    position: absolute;
    inset: 0;
    z-index: 20;
    overflow: hidden;
    background: var(--body-background);
    animation: sheet-in 220ms ease-out;
  }
  @keyframes sheet-in {
    from {
      opacity: 0;
    }
  }
  .sheet-art {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: center 25%;
  }
  .sheet-fade {
    position: absolute;
    inset: 0;
    background:
      linear-gradient(to right, color-mix(in srgb, var(--body-background) 92%, transparent) 30%, transparent 75%),
      linear-gradient(to top, var(--body-background) 4%, transparent 50%);
  }
  .sheet-body {
    position: absolute;
    left: 7vw;
    bottom: 12vh;
    width: min(620px, 46vw);
    display: flex;
    flex-direction: column;
    gap: 28px;
  }
  .sheet-logo {
    max-width: 100%;
    max-height: 170px;
    object-fit: contain;
    object-position: left bottom;
    filter: drop-shadow(0 6px 24px rgba(0, 0, 0, 0.6));
  }
  .sheet-title {
    margin: 0;
    font-size: 52px;
    line-height: 1.05;
  }
  .sheet-stats {
    display: flex;
    flex-wrap: wrap;
    gap: 14px 36px;
  }
  .sheet-note {
    max-width: 60ch;
    margin: 0;
    font-size: 1.05rem;
    line-height: 1.45;
    opacity: 0.85;
  }
  .st {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .st .k {
    font-size: 13px;
    letter-spacing: 1px;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }
  .st .v {
    font-size: 24px;
    font-weight: 600;
  }
  .st .v.small {
    font-size: 16px;
    font-weight: 400;
    color: var(--text-secondary);
    word-break: break-all;
  }
  .st .v.err {
    font-size: 16px;
    color: var(--danger);
  }
  .calc {
    display: inline-flex;
    align-items: center;
    gap: 12px;
    font-size: 20px !important;
    font-weight: 400 !important;
    color: var(--text-secondary);
  }
  .spin {
    width: 18px;
    height: 18px;
    border: 3px solid rgba(255, 255, 255, 0.2);
    border-top-color: var(--accent-overlay);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .sheet-actions {
    display: flex;
    gap: 14px;
  }
  .big-install,
  .big-back {
    display: inline-flex;
    align-items: center;
    gap: 12px;
    height: 60px;
    padding: 0 30px;
    border-radius: 12px;
    font-size: 20px;
    font-weight: 600;
    outline: none;
    transition: transform 140ms ease, box-shadow 140ms ease;
  }
  .big-install {
    background: var(--accent);
    color: var(--accent-text);
  }
  .big-back {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text-default);
  }
  .big-install:focus,
  .big-back:focus {
    transform: scale(1.04);
    box-shadow: 0 0 0 3px var(--console-focus);
  }
  .overlay-card h2 {
    font-size: 28px;
    margin: 0 0 12px;
  }
  .small {
    font-size: 14px;
  }
  .hold {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .hold-line {
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .hold-line kbd {
    padding: 2px 8px;
    border: 1px solid rgba(255, 255, 255, 0.35);
    border-radius: 6px;
    font: inherit;
    font-size: 0.85em;
    font-weight: 600;
  }
  .hold-bar {
    width: 240px;
    height: 6px;
    border-radius: 3px;
    background: rgba(255, 255, 255, 0.15);
    overflow: hidden;
  }
  .hold-fill {
    display: block;
    height: 100%;
    background: var(--danger);
    animation: fill 2s linear forwards;
  }
  @keyframes fill {
    from {
      width: 0;
    }
    to {
      width: 100%;
    }
  }
  .chord {
    position: absolute;
    top: 28px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 40;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 20px;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.6);
    backdrop-filter: blur(8px);
    font-size: 16px;
  }
  .chord-ring {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: conic-gradient(var(--danger) calc(var(--c) * 360deg), rgba(255, 255, 255, 0.15) 0);
  }
  .toast {
    position: absolute;
    left: 50%;
    bottom: 90px;
    transform: translateX(-50%);
    padding: 12px 22px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.12);
    backdrop-filter: blur(8px);
    font-size: 16px;
    z-index: 30;
  }
  .spinner {
    width: 48px;
    height: 48px;
    margin: 0 auto 18px;
    border: 4px solid rgba(255, 255, 255, 0.2);
    border-top-color: #fff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .spinner.idle {
    animation-duration: 2.4s;
    opacity: 0.6;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
