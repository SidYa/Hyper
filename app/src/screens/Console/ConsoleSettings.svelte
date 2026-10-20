<script lang="ts">
  // Console-mode settings, laid out like Steam Big Picture: categories on the
  // left, the category's options on the right. Everything is reachable with
  // the d-pad: Left/Right switch choices, A toggles, B steps back
  // out (options → categories → library).
  import { tick } from 'svelte'
  import { settings, updateSettings } from '$lib/state/settings.svelte'
  import { SUPPORTED_LOCALES, t } from '$lib/i18n/index.svelte'
  import { pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { errorMessage, storeLoginStart, storeLogout } from '$lib/api/commands'
  import { host } from '$lib/state/host.svelte'
  import { libraryStores, libraryTab, storeOf } from '$lib/state/library.svelte'
  import { notify } from '$lib/state/toast.svelte'
  import { ACCENT_PRESETS } from '$lib/theme'
  import { RUNNER_NAMES, RUNNERS, type AppSettings, type Runner } from '$types/index'
  import ConsolePage from './ConsolePage.svelte'
  import ConsoleDialog from './ConsoleDialog.svelte'
  import ConsoleFolderPicker from './ConsoleFolderPicker.svelte'

  interface Props {
    onback: () => void
  }
  let { onback }: Props = $props()

  type Key = keyof AppSettings
  type Option = { value: string; label: string }
  type Row =
    | { kind: 'toggle'; key: Key; label: string; note?: string }
    | { kind: 'choice'; key: Key; label: string; options: Option[] }
    // colour circles; Left/Right (or A) pick the neighbour
    | { kind: 'swatches'; key: Key; label: string; options: Option[] }
    | { kind: 'info'; label: string; value: string }
    | { kind: 'action'; label: string; value?: string; button: string; danger?: boolean; run: () => void }
  type Category = { id: string; label: string; icon: string; rows: Row[] }

  // the setting whose folder picker is open
  type FolderKey = 'defaultInstallPath' | 'winePrefixesDir'
  let pickingFolder = $state<FolderKey | null>(null)
  function pickFolder(path: string) {
    const key = pickingFolder
    pickingFolder = null
    if (key) set(key, path)
  }

  // Linux: games start through umu + UMU-Proton (core/src/compat.rs)
  const prefixesDefault = $derived(`${settings.value.defaultInstallPath}/Prefixes`)

  // store whose logout waits for confirmation
  let confirmLogout = $state<Runner | null>(null)
  function doLogout() {
    const r = confirmLogout
    confirmLogout = null
    if (r) storeLogout(r).catch((e) => notify(errorMessage(e), 'error'))
  }
  // The store's login page opens maximized above the console UI; holding
  // Select + Start closes it (gamepad_watch.rs).
  function login(r: Runner) {
    storeLoginStart(r, true).catch((e) => notify(errorMessage(e), 'error'))
  }

  const s = $derived(settings.value)
  const storeLabel = (r: string) => (r === 'legendary' ? 'Epic' : RUNNER_NAMES[r as keyof typeof RUNNER_NAMES])

  const categories = $derived<Category[]>([
    {
      id: 'general',
      label: t('settings.general', 'General'),
      icon: 'M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7zM12 2v3m0 14v3M2 12h3m14 0h3M4.9 4.9l2.1 2.1m10 10 2.1 2.1M4.9 19.1 7 17M17 7l2.1-2.1',
      rows: [
        { kind: 'choice', key: 'language', label: t('settings.language', 'Language'), options: SUPPORTED_LOCALES.map((l) => ({ value: l.code, label: l.name })) },
        { kind: 'toggle', key: 'startInConsoleMode', label: t('settings.startInConsoleMode', 'Start in console mode') },
        { kind: 'toggle', key: 'autoSyncSaves', label: t('saves.autoSync', 'Sync cloud saves automatically') },
        { kind: 'info', label: '', value: t('saves.autoSyncHint', 'Download newer saves before a game starts and upload them after it closes.') },
        ...(host.alwaysConsole
          ? []
          : [
              { kind: 'toggle', key: 'exitToTray', label: t('settings.exitToTray', 'Minimize to tray') } as Row,
              { kind: 'info', label: '', value: t('settings.exitToTrayHint', 'Closing the window hides Hyper in the tray instead of quitting; downloads carry on.') } as Row
            ]),
        { kind: 'action', label: t('settings.defaultInstallPath', 'Games Lib'), value: s.defaultInstallPath || '—', button: t('console.settings.change', 'Change'), run: () => (pickingFolder = 'defaultInstallPath') }
      ]
    },
    {
      id: 'interface',
      label: t('settings.interface', 'Interface'),
      icon: 'M12 3a9 9 0 1 0 0 18c1 0 1.5-.7 1.5-1.5 0-.4-.2-.8-.4-1.1-.3-.3-.4-.7-.4-1.1 0-.8.7-1.5 1.5-1.5H16a5 5 0 0 0 5-5c0-4.4-4-7.8-9-7.8zM7.5 12.5h.01M9.5 8h.01M14.5 8h.01',
      rows: [
        { kind: 'swatches', key: 'accentColor', label: t('settings.accentLabel', 'Accent colour'), options: [
          ...ACCENT_PRESETS.map((p) => ({ value: p.color, label: t('settings.accent.' + p.id, p.id) })),
          ...(ACCENT_PRESETS.some((p) => p.color.toLowerCase() === s.accentColor.toLowerCase())
            ? []
            : [{ value: s.accentColor, label: t('settings.accent.custom', 'Custom') }])
        ] }
      ]
    },
    {
      id: 'library',
      label: t('settings.library', 'Library'),
      icon: 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z',
      rows: [
        { kind: 'choice', key: 'consoleModeDefaultTab', label: t('settings.consoleModeDefaultTab', 'Default tab'), options: [
          ...(libraryStores().length >= 2 ? [{ value: 'all', label: t('console.all', 'All') }] : []),
          { value: 'installed', label: t('console.installed', 'Installed') },
          ...libraryStores().map((r) => ({ value: r, label: storeLabel(r) }))
        ] },
        { kind: 'toggle', key: 'libraryInstalledFirst', label: t('settings.libraryInstalledFirst', 'Installed games first') },
        { kind: 'toggle', key: 'libraryGrayscaleUninstalled', label: t('settings.libraryGrayscale', 'Black-and-white covers for games that are not installed') },
        { kind: 'toggle', key: 'hideDuplicates', label: t('settings.hideDuplicates', 'Hide duplicate games owned in both stores') },
        ...(s.hideDuplicates
          ? [{ kind: 'choice', key: 'hideDuplicatesFrom', label: t('settings.hideDuplicatesFrom', 'Hide the copy from'), options: RUNNERS.map((r) => ({ value: r, label: storeLabel(r) })) } as Row]
          : [])
      ]
    },
    ...(host.linux
      ? [
          {
            id: 'proton',
            label: 'Proton',
            icon: 'M9 3h6M10 3v6l-5.4 9.2A1.9 1.9 0 0 0 6.2 21h11.6a1.9 1.9 0 0 0 1.6-2.8L14 9V3M7.5 15h9',
            rows: [
              { kind: 'action', label: t('settings.winePrefixesDir', 'Wine prefixes'), value: s.winePrefixesDir || prefixesDefault, button: t('console.settings.change', 'Change'), run: () => (pickingFolder = 'winePrefixesDir') },
              { kind: 'info', label: '', value: t('console.settings.protonHint', 'Windows games start through Proton. The first start downloads the Steam runtime and Proton (about 1 GB) and can take several minutes.') }
            ]
          } satisfies Category
        ]
      : []),
    {
      id: 'tiles',
      label: t('settings.group.tiles', 'Tiles'),
      icon: 'M5 3h14v18H5zM5 15h14',
      rows: [
        { kind: 'toggle', key: 'consoleModeShowCardTitles', label: t('settings.consoleModeShowCardTitles', 'Titles under tiles') },
        ...(s.consoleModeShowCardTitles
          ? [{ kind: 'toggle', key: 'consoleModeShowCardStore', label: t('settings.consoleModeShowCardStore', 'Store name under the title') } as Row]
          : []),
        { kind: 'toggle', key: 'consoleShowProgressPercent', label: t('settings.consoleShowProgressPercent', 'Install percentage on the tile') }
      ]
    },
    {
      id: 'controller',
      label: t('settings.group.controller', 'Controller'),
      icon: 'M7 8h10a5 5 0 0 1 4.6 6.9l-.9 2.2a2.3 2.3 0 0 1-3.9.6L15 16H9l-1.8 1.7a2.3 2.3 0 0 1-3.9-.6l-.9-2.2A5 5 0 0 1 7 8zM8 11v3m-1.5-1.5h3M15.5 12.5h.01M17.5 11h.01',
      rows: [
        { kind: 'choice', key: 'consoleModeKeyboardLayout', label: t('settings.consoleModeKeyboardLayout', 'On-screen keyboard'), options: [
          { value: 'tvos', label: t('settings.kb.tvos', 'tvOS carousel') },
          { value: 'qwerty', label: 'QWERTY' }
        ] }
      ]
    },
    {
      id: 'sound',
      label: t('settings.group.sound', 'Sound'),
      icon: 'M4 9h4l5-4v14l-5-4H4zM16.5 8.5a5 5 0 0 1 0 7',
      rows: [
        { kind: 'toggle', key: 'consoleModeSounds', label: t('settings.consoleModeSounds', 'Interface sounds') }
      ]
    },
    {
      id: 'accounts',
      label: t('settings.accounts', 'Accounts'),
      icon: 'M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM4 21a8 8 0 0 1 16 0',
      rows: [
        ...RUNNERS.map((r): Row => {
          const st = storeOf(r)
          return st?.loggedIn
            ? { kind: 'action', label: storeLabel(r), value: st.username ?? '', button: t('login.logout', 'Log out'), danger: true, run: () => (confirmLogout = r) }
            : { kind: 'action', label: storeLabel(r), value: t('settings.notLoggedIn', 'not logged in'), button: t('login.title', 'Log in'), run: () => login(r) }
        }),
        { kind: 'info', label: '', value: t('console.accounts.hint', "Logging in opens the store’s page. Tap the fields or use the mouse; on the Steam Deck Steam + X opens the keyboard. Hold Select + Start for 2 seconds to cancel.") }
      ]
    }
  ])

  let catIndex = $state(0)
  let rowIndex = $state(0)
  let zone = $state<'cats' | 'rows'>('cats')
  let rowEls = $state<Array<HTMLElement | undefined>>([])

  const category = $derived(categories[catIndex])
  const focusable = $derived(category.rows.map((r, i) => (r.kind === 'info' ? -1 : i)).filter((i) => i >= 0))

  function value(key: Key) {
    // a default tab that is not there (one store, or logged out) shows as
    // the tab console mode actually opens
    if (key === 'consoleModeDefaultTab') return libraryTab(s.consoleModeDefaultTab)
    return s[key]
  }
  function set(key: Key, v: unknown) {
    void updateSettings({ [key]: v } as Partial<AppSettings>)
  }

  function adjust(row: Row, delta: 1 | -1) {
    if (row.kind === 'toggle') set(row.key, !value(row.key))
    else if (row.kind === 'choice' || row.kind === 'swatches') {
      const i = row.options.findIndex((o) => o.value === value(row.key))
      const next = row.options[(i + delta + row.options.length) % row.options.length]
      set(row.key, next.value)
    } else if (row.kind === 'action') row.run()
  }

  function enterRows() {
    if (!focusable.length) return
    zone = 'rows'
    rowIndex = focusable[0]
  }
  function moveRow(delta: 1 | -1) {
    const pos = focusable.indexOf(rowIndex)
    const next = focusable[pos + delta]
    if (next !== undefined) rowIndex = next
  }

  $effect(() => {
    void rowIndex
    void zone
    tick().then(() => rowEls[rowIndex]?.scrollIntoView({ block: 'nearest' }))
  })

  $effect(() =>
    pushHandler((action) => {
      const dir = directionOf(action)
      if (pickingFolder) return true // the picker has its own handler on top
      if (confirmLogout) {
        if (action === 'mainAction') doLogout()
        else if (action === 'back') confirmLogout = null
        return true
      }
      if (zone === 'cats') {
        if (dir === 'up') catIndex = Math.max(0, catIndex - 1)
        else if (dir === 'down') catIndex = Math.min(categories.length - 1, catIndex + 1)
        else if (dir === 'right' || action === 'mainAction') enterRows()
        else if (action === 'back') onback()
        return true
      }
      const row = category.rows[rowIndex]
      if (dir === 'up') moveRow(-1)
      else if (dir === 'down') moveRow(1)
      else if (dir === 'left') {
        if (row.kind === 'choice' || row.kind === 'swatches') adjust(row, -1)
        else zone = 'cats'
      } else if (dir === 'right') {
        if (row.kind === 'choice' || row.kind === 'swatches') adjust(row, 1)
      } else if (action === 'mainAction') adjust(row, 1)
      else if (action === 'back') zone = 'cats'
      else if (action === 'prevPage' || action === 'nextPage') {
        catIndex = Math.min(categories.length - 1, Math.max(0, catIndex + (action === 'nextPage' ? 1 : -1)))
        zone = 'cats'
      }
      return true
    })
  )

  const hints = $derived(
    zone === 'cats'
      ? [
          { buttons: ['main' as const], label: t('console.select', 'Select') },
          { buttons: ['back' as const], label: t('console.back', 'Back') },
          { buttons: ['dpad' as const], label: t('console.navigate', 'Navigate') }
        ]
      : [
          { buttons: ['main' as const], label: t('console.settings.change', 'Change') },
          { buttons: ['dpad' as const], label: t('console.settings.adjust', 'Adjust') },
          { buttons: ['back' as const], label: t('console.back', 'Back') }
        ]
  )
</script>

<ConsolePage title={t('settings.title', 'Settings')} {hints}>
  <div class="layout">
    <ul class="cats">
      {#each categories as c, i (c.id)}
        <li>
          <button
            class="cat"
            class:on={i === catIndex}
            class:focused={zone === 'cats' && i === catIndex}
            onclick={() => {
              catIndex = i
              zone = 'cats'
            }}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d={c.icon} /></svg>
            {c.label}
          </button>
        </li>
      {/each}
    </ul>

    <div class="rows" class:active={zone === 'rows'}>
      <h2>{category.label}</h2>
      {#each category.rows as row, i (i)}
        {#if row.kind === 'info'}
          <div class="row info" class:note={!row.label}>
            {#if row.label}<span class="label">{row.label}</span>{/if}
            <span class="value">{row.value}</span>
          </div>
        {:else}
          <button
            class="row"
            class:focused={zone === 'rows' && i === rowIndex}
            bind:this={rowEls[i]}
            onclick={() => {
              zone = 'rows'
              rowIndex = i
              adjust(row, 1)
            }}
          >
            <span class="label">
              {row.label}
              {#if row.kind === 'toggle' && row.note}<span class="soon">{row.note}</span>{/if}
            </span>
            {#if row.kind === 'toggle'}
              <span class="switch" class:on={!!value(row.key)}><span class="knob"></span></span>
            {:else if row.kind === 'swatches'}
              <span class="swatches">
                {#each row.options as o (o.value)}
                  <span
                    class="swatch"
                    class:on={o.value.toLowerCase() === String(value(row.key)).toLowerCase()}
                    style:--c={o.value}
                    title={o.label}
                  ></span>
                {/each}
              </span>
            {:else if row.kind === 'choice'}
              <span class="choice">
                <span class="arrow">‹</span>
                {row.options.find((o) => o.value === value(row.key))?.label ?? String(value(row.key))}
                <span class="arrow">›</span>
              </span>
            {:else}
              <span class="value-text">{row.value}</span>
              <span class="pill" class:danger={row.danger}>{row.button}</span>
            {/if}
          </button>
        {/if}
      {/each}
    </div>
  </div>
</ConsolePage>

{#if pickingFolder}
  <ConsoleFolderPicker
    title={pickingFolder === 'winePrefixesDir' ? t('settings.winePrefixesDir', 'Wine prefixes') : t('settings.defaultInstallPath', 'Games Lib')}
    start={pickingFolder === 'winePrefixesDir' ? s.winePrefixesDir || s.defaultInstallPath : s.defaultInstallPath}
    onpick={pickFolder}
    oncancel={() => (pickingFolder = null)}
  />
{/if}

{#if confirmLogout}
  <ConsoleDialog
    title={t('login.confirmLogout', 'Log out of {{store}}?', { store: storeLabel(confirmLogout) })}
    confirmLabel={t('login.logout', 'Log out')}
    cancelLabel={t('console.keep', 'Keep')}
    danger
    onconfirm={doLogout}
    oncancel={() => (confirmLogout = null)}
  />
{/if}

<style>
  .layout {
    height: 100%;
    display: grid;
    grid-template-columns: 300px 1fr;
    gap: 40px;
  }
  .cats {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cat {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 16px;
    height: 54px;
    padding: 0 18px;
    border-radius: 10px;
    font-size: 19px;
    color: var(--text-secondary);
    text-align: left;
    outline: none;
  }
  .cat.on {
    background: rgba(255, 255, 255, 0.08);
    color: var(--text-default);
  }
  .cat.focused {
    background: var(--text-default);
    color: var(--background-darker);
  }
  .cat svg {
    width: 24px;
    height: 24px;
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .rows {
    min-height: 0;
    overflow-y: auto;
    scrollbar-width: none;
    /* room for the 3px focus ring: a scroll container clips box-shadow */
    padding: 6px 8px 40px 6px;
    opacity: 0.75;
    transition: opacity 140ms ease;
  }
  .rows.active {
    opacity: 1;
  }
  h2 {
    margin: 8px 0 18px;
    font-size: 24px;
    font-weight: 600;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 24px;
    min-height: 64px;
    padding: 10px 22px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.04);
    margin-bottom: 6px;
    font-size: 19px;
    color: var(--text-default);
    text-align: left;
    outline: none;
    transition: box-shadow 120ms ease, background 120ms ease;
  }
  .row.focused {
    background: rgba(255, 255, 255, 0.1);
    box-shadow: 0 0 0 3px var(--console-focus);
  }
  .row.info {
    background: none;
    color: var(--text-secondary);
  }
  .row.info .value {
    color: var(--text-default);
    text-align: right;
    overflow-wrap: anywhere;
  }
  .row.note {
    min-height: 0;
    font-size: 15px;
  }
  .row.note .value {
    color: var(--text-tertiary);
  }
  .label {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .soon {
    padding: 2px 8px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    font-size: 13px;
    color: var(--text-secondary);
  }
  .switch {
    position: relative;
    flex: none;
    width: 52px;
    height: 30px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.18);
    transition: background 140ms ease;
  }
  .switch.on {
    background: var(--accent);
  }
  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    background: #fff;
    transition: transform 140ms ease;
  }
  .switch.on .knob {
    transform: translateX(22px);
  }
  .choice {
    display: inline-flex;
    align-items: center;
    gap: 14px;
    flex: none;
    font-weight: 500;
  }
  .value-text {
    margin-left: auto;
    color: var(--text-secondary);
  }
  .pill {
    flex: none;
    padding: 6px 16px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.1);
    font-size: 16px;
    font-weight: 500;
  }
  .pill.danger {
    color: var(--danger);
  }
  .row.focused .pill.danger {
    background: var(--danger);
    color: #fff;
  }
  .swatches {
    display: inline-flex;
    align-items: center;
    gap: 12px;
    flex: none;
  }
  .swatch {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    background: var(--c);
    box-shadow: inset 0 0 0 2px rgba(255, 255, 255, 0.12);
    transition: transform 140ms ease, box-shadow 140ms ease;
  }
  /* the chosen colour: a gap ring, like the desktop swatches */
  .swatch.on {
    transform: scale(1.12);
    box-shadow:
      0 0 0 3px var(--background-darker),
      0 0 0 6px var(--c);
  }
  .row:not(.focused) .swatch:not(.on) {
    opacity: 0.55;
  }
  .arrow {
    font-size: 24px;
    line-height: 1;
    color: var(--text-tertiary);
  }
  .row.focused .arrow {
    color: var(--text-default);
  }
</style>
