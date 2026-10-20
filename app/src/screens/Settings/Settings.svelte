<script lang="ts">
  import { settings, updateSettings } from '$lib/state/settings.svelte'
  import { host } from '$lib/state/host.svelte'
  import { SUPPORTED_LOCALES, t } from '$lib/i18n/index.svelte'
  import { open as openDialog } from '@tauri-apps/plugin-dialog'
  import { askConfirm } from '$lib/state/confirm.svelte'
  import { ACCENT_PRESETS } from '$lib/theme'
  import Select from '$lib/components/Select.svelte'
  import LicensesDialog from '$lib/components/LicensesDialog.svelte'
  import { openAppFolder, openExternal } from '$lib/api/commands'
  import { errorMessage, storeLoginStart, storeLogout } from '$lib/api/commands'
  import { libraryStores, libraryTab, storeOf } from '$lib/state/library.svelte'
  import { notify } from '$lib/state/toast.svelte'
  import type { Runner } from '$types/index'
  import { RUNNER_NAMES, RUNNERS, type ConsoleModeDefaultTab } from '$types/index'

  interface Props {
    section?: string
  }
  let { section = 'general' }: Props = $props()

  // version, folders, edition: fetched once at boot (host.svelte.ts)
  const info = $derived(host.info)
  const report = (e: unknown) => notify(errorMessage(e), 'error')

  const s = $derived(settings.value)

  // /settings/<section> scrolls straight to that tile
  $effect(() => {
    if (section && section !== 'general') document.getElementById(`settings-${section}`)?.scrollIntoView({ block: 'start', behavior: 'smooth' })
  })

  async function pickInstallPath() {
    const dir = await openDialog({ directory: true, defaultPath: s.defaultInstallPath || undefined })
    if (typeof dir === 'string') await updateSettings({ defaultInstallPath: dir })
  }

  const isPreset = $derived(ACCENT_PRESETS.some((p) => p.color.toLowerCase() === s.accentColor.toLowerCase()))
  let customInput = $state<HTMLInputElement | undefined>()

  // Fixed arrangement per column count (CSS multi-column balancing left a
  // whole column empty once the Console tile grew).
  type SectionId = 'general' | 'interface' | 'accounts' | 'library' | 'console' | 'about'
  const LAYOUTS: Record<1 | 2 | 3, SectionId[][]> = {
    3: [['general', 'interface', 'accounts'], ['library', 'console'], ['about']],
    2: [['general', 'interface', 'accounts', 'about'], ['library', 'console']],
    1: [['general', 'interface', 'accounts', 'library', 'console', 'about']]
  }
  let width = $state(0)
  const colCount = $derived<1 | 2 | 3>(width >= 1380 ? 3 : width >= 900 ? 2 : 1)
  const layout = $derived(LAYOUTS[colCount])

  let showLicenses = $state(false)

  // PayPal donation page for the author; the address is only shown by PayPal
  // itself after the button opens the system browser.
  const DONATE_URL = 'https://www.paypal.com/donate/?business=' + encodeURIComponent('sergiisidko@gmail.com') + '&no_recurring=0&currency_code=USD'

  let busy = $state<Runner | null>(null)
  async function account(runner: Runner) {
    busy = runner
    try {
      if (storeOf(runner)?.loggedIn) {
        const ok = await askConfirm({
          title: t('login.confirmLogout', 'Log out of {{store}}?', { store: RUNNER_NAMES[runner] }),
          subject: storeOf(runner)?.username ?? undefined,
          confirmLabel: t('login.logout', 'Log out'),
          cancelLabel: t('game.cancel', 'Cancel')
        })
        if (ok) await storeLogout(runner)
      } else {
        await storeLoginStart(runner)
      }
    } catch (e) {
      notify(errorMessage(e), 'error')
    } finally {
      busy = null
    }
  }
  // only tabs console mode can open: "All" with two or more stores
  const tabs = $derived<Array<{ value: ConsoleModeDefaultTab; label: string }>>([
    ...(libraryStores().length >= 2 ? [{ value: 'all' as const, label: t('console.all', 'All') }] : []),
    { value: 'installed', label: t('console.installed', 'Installed') },
    ...libraryStores().map((r) => ({ value: r, label: RUNNER_NAMES[r] }))
  ])
</script>

<div class="settings" bind:clientWidth={width}>
  <h1>{t('settings.title', 'Settings')}</h1>

  {#snippet sec_general()}
  <section id="settings-general">
    <h2>{t('settings.general', 'General')}</h2>
    <div class="row">
      <span>{t('settings.language', 'Language')}</span>
      <Select
        label={t('settings.language', 'Language')}
        value={s.language}
        options={SUPPORTED_LOCALES.map((l) => ({ value: l.code, label: l.name }))}
        onchange={(v) => updateSettings({ language: v })}
      />
    </div>
    <label class="row">
      <span>{t('settings.defaultInstallPath', 'Games Lib')}</span>
      <div class="path">
        <input value={s.defaultInstallPath} onchange={(e) => updateSettings({ defaultInstallPath: e.currentTarget.value })} />
        <button class="btn" type="button" onclick={pickInstallPath}>{t('game.browse', 'Browse…')}</button>
      </div>
    </label>
    <label class="row">
      <span>
        {t('saves.autoSync', 'Sync cloud saves automatically')}
        <small class="hint">{t('saves.autoSyncHint', 'Download newer saves before a game starts and upload them after it closes.')}</small>
      </span>
      <input type="checkbox" checked={s.autoSyncSaves} onchange={(e) => updateSettings({ autoSyncSaves: e.currentTarget.checked })} />
    </label>
    {#if info && !info.alwaysConsole}
      <label class="row">
        <span>
          {t('settings.exitToTray', 'Minimize to tray')}
          <small class="hint">{t('settings.exitToTrayHint', 'Closing the window hides Hyper in the tray instead of quitting; downloads carry on.')}</small>
        </span>
        <input type="checkbox" checked={s.exitToTray} onchange={(e) => updateSettings({ exitToTray: e.currentTarget.checked })} />
      </label>
    {/if}
  </section>
  {/snippet}

  {#snippet sec_interface()}
  <section id="settings-interface">
    <h2>{t('settings.interface', 'Interface')}</h2>
    <div class="row">
      <span>{t('settings.appearance', 'Theme')}</span>
      <div class="seg">
        {#each [['system', t('settings.appearanceSystem', 'System')], ['light', t('settings.appearanceLight', 'Light')], ['dark', t('settings.appearanceDark', 'Dark')]] as [value, label] (value)}
          <button
            type="button"
            class:on={s.appearance === value}
            onclick={() => updateSettings({ appearance: value as 'system' | 'light' | 'dark' })}
          >{label}</button>
        {/each}
      </div>
    </div>
    <div class="row">
      <span>{t('settings.accentLabel', 'Accent colour')}</span>
      <div class="swatches">
        {#each ACCENT_PRESETS as p (p.id)}
          <button
            type="button"
            class="swatch"
            class:on={s.accentColor.toLowerCase() === p.color.toLowerCase()}
            style:--c={p.color}
            title={t('settings.accent.' + p.id, p.id)}
            aria-label={t('settings.accent.' + p.id, p.id)}
            onclick={() => updateSettings({ accentColor: p.color })}
          ></button>
        {/each}
        <button
          type="button"
          class="swatch custom"
          class:on={!isPreset}
          style:--c={isPreset ? 'transparent' : s.accentColor}
          title={t('settings.accent.custom', 'Custom')}
          aria-label={t('settings.accent.custom', 'Custom')}
          onclick={() => customInput?.click()}
        ></button>
        <input
          class="color-input"
          type="color"
          bind:this={customInput}
          value={s.accentColor}
          oninput={(e) => updateSettings({ accentColor: e.currentTarget.value })}
          tabindex="-1"
        />
      </div>
    </div>
  </section>
  {/snippet}


  {#snippet sec_accounts()}
  <section id="settings-accounts">
    <h2>{t('settings.accounts', 'Accounts')}</h2>
    {#each RUNNERS as r (r)}
      {@const st = storeOf(r)}
      <div class="row">
        <span>{RUNNER_NAMES[r]} <span class="muted small">· {st?.loggedIn ? (st.username ?? '') : t('settings.notLoggedIn', 'not logged in')}</span></span>
        <button class="btn" class:danger={st?.loggedIn} disabled={busy === r} onclick={() => account(r)}>
          {st?.loggedIn ? t('login.logout', 'Log out') : t('login.title', 'Log in')}
        </button>
      </div>
    {/each}
  </section>
  {/snippet}

  {#snippet sec_library()}
  <section id="settings-library">
    <h2>{t('settings.library', 'Library')}</h2>
    <label class="row">
      <span>{t('settings.libraryInstalledFirst', 'Installed games first')}</span>
      <input type="checkbox" checked={s.libraryInstalledFirst} onchange={(e) => updateSettings({ libraryInstalledFirst: e.currentTarget.checked })} />
    </label>
    <label class="row">
      <span>{t('settings.libraryGrayscale', 'Black-and-white covers for games that are not installed')}</span>
      <input type="checkbox" checked={s.libraryGrayscaleUninstalled} onchange={(e) => updateSettings({ libraryGrayscaleUninstalled: e.currentTarget.checked })} />
    </label>
    <label class="row">
      <span>{t('settings.hideDuplicates', 'Hide duplicate games owned in both stores')}</span>
      <input type="checkbox" checked={s.hideDuplicates} onchange={(e) => updateSettings({ hideDuplicates: e.currentTarget.checked })} />
    </label>
    {#if s.hideDuplicates}
      <div class="row">
        <span>{t('settings.hideDuplicatesFrom', 'Hide the copy from')}</span>
        <Select
          label={t('settings.hideDuplicatesFrom', 'Hide the copy from')}
          value={s.hideDuplicatesFrom}
          options={RUNNERS.map((r) => ({ value: r, label: RUNNER_NAMES[r] }))}
          onchange={(v) => updateSettings({ hideDuplicatesFrom: v })}
        />
      </div>
    {/if}
  </section>
  {/snippet}

  {#snippet sec_console()}
  <section id="settings-console">
    <h2>{t('settings.consoleMode', 'Console mode')}</h2>

    <h3>{t('settings.group.startup', 'Startup')}</h3>
    <label class="row">
      <span>{t('settings.startInConsoleMode', 'Start in console mode')}</span>
      <input type="checkbox" checked={s.startInConsoleMode} onchange={(e) => updateSettings({ startInConsoleMode: e.currentTarget.checked })} />
    </label>
    <div class="row">
      <span>{t('settings.consoleModeDefaultTab', 'Default tab')}</span>
      <Select
        label={t('settings.consoleModeDefaultTab', 'Default tab')}
        value={libraryTab(s.consoleModeDefaultTab)}
        options={tabs}
        onchange={(v) => updateSettings({ consoleModeDefaultTab: v })}
      />
    </div>

    <h3>{t('settings.group.tiles', 'Tiles')}</h3>
    <label class="row">
      <span>{t('settings.consoleModeShowCardTitles', 'Titles under tiles')}</span>
      <input type="checkbox" checked={s.consoleModeShowCardTitles} onchange={(e) => updateSettings({ consoleModeShowCardTitles: e.currentTarget.checked })} />
    </label>
    {#if s.consoleModeShowCardTitles}
      <label class="row">
        <span>{t('settings.consoleModeShowCardStore', 'Store name under the title')}</span>
        <input type="checkbox" checked={s.consoleModeShowCardStore} onchange={(e) => updateSettings({ consoleModeShowCardStore: e.currentTarget.checked })} />
      </label>
    {/if}
    <label class="row">
      <span>{t('settings.consoleShowProgressPercent', 'Install percentage on the tile')}</span>
      <input type="checkbox" checked={s.consoleShowProgressPercent} onchange={(e) => updateSettings({ consoleShowProgressPercent: e.currentTarget.checked })} />
    </label>

    <h3>{t('settings.group.controller', 'Controller')}</h3>
    <div class="row">
      <span>{t('settings.consoleModeKeyboardLayout', 'On-screen keyboard')}</span>
      <Select
        label={t('settings.consoleModeKeyboardLayout', 'On-screen keyboard')}
        value={s.consoleModeKeyboardLayout}
        options={[
          { value: 'tvos' as const, label: t('settings.kb.tvos', 'tvOS carousel') },
          { value: 'qwerty' as const, label: 'QWERTY' }
        ]}
        onchange={(v) => updateSettings({ consoleModeKeyboardLayout: v })}
      />
    </div>

    <h3>{t('settings.group.sound', 'Sound')}</h3>
    <label class="row">
      <span>{t('settings.consoleModeSounds', 'Interface sounds')}</span>
      <input type="checkbox" checked={s.consoleModeSounds} onchange={(e) => updateSettings({ consoleModeSounds: e.currentTarget.checked })} />
    </label>
  </section>
  {/snippet}

  {#snippet sec_about()}
  <section id="settings-about">
    <h2>{t('settings.about', 'About')}</h2>
    {#if info}
      <div class="row"><span>{t('settings.version', 'Version')}</span><code>{info.version} ({info.platform})</code></div>
      <div class="row">
        <span>{t('settings.configDir', 'Config folder')}</span>
        <button class="path-link" type="button" title={t('settings.openFolder', 'Open folder')} onclick={() => openAppFolder('config').catch(report)}>{info.configDir}</button>
      </div>
      <div class="row">
        <span>{t('settings.logsDir', 'Logs folder')}</span>
        <button class="path-link" type="button" title={t('settings.openFolder', 'Open folder')} onclick={() => openAppFolder('logs').catch(report)}>{info.logsDir}</button>
      </div>
    {/if}
    <div class="about-actions">
      <button class="btn mono" type="button" onclick={() => (showLicenses = true)}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 3h7l4 4v14H7z" /><path d="M14 3v4h4M10 12h5M10 16h5" /></svg>
        {t('settings.licenses', 'Licenses')}
      </button>
      <button class="btn mono" type="button" onclick={() => openExternal(DONATE_URL).catch(report)}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z" /></svg>
        {t('settings.donatePaypal', 'Donate with PayPal')}
      </button>
    </div>
  </section>
  {/snippet}

  <div class="cols cols-{colCount}">
    {#each layout as column, ci (ci)}
      <div class="col">
        {#each column as id (id)}
          {#if id === 'general'}{@render sec_general()}
          {:else if id === 'interface'}{@render sec_interface()}
          {:else if id === 'accounts'}{@render sec_accounts()}
          {:else if id === 'library'}{@render sec_library()}
          {:else if id === 'console'}{@render sec_console()}
          {:else if id === 'about'}{@render sec_about()}{/if}
        {/each}
      </div>
    {/each}
  </div>

  {#if showLicenses}
    <LicensesDialog onclose={() => (showLicenses = false)} />
  {/if}

</div>

<style>
  .settings {
    padding: var(--space-lg) var(--space-xl);
  }
  .cols {
    display: grid;
    gap: var(--space-md);
    align-items: start;
  }
  .cols-1 {
    grid-template-columns: minmax(0, 1fr);
  }
  .cols-2 {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .cols-3 {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: var(--space-md);
    min-width: 0;
  }
  h1 {
    margin: 0 0 var(--space-lg);
    font-size: 26px;
  }
  /* Each tile is a grid whose label column is as wide as its longest label
     (capped), and every row shares it via subgrid, so the controls inside a
     tile line up on one edge. */
  section {
    display: grid;
    grid-template-columns: fit-content(56%) minmax(0, 1fr);
    column-gap: var(--space-md);
    width: 100%;
    background: var(--background);
    border-radius: 12px;
    padding: var(--space-md) var(--space-lg);
    margin: 0;
  }
  h2 {
    font-size: 16px;
    margin: 0 0 var(--space-sm);
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.6px;
  }
  section > :global(*) {
    grid-column: 1 / -1;
  }
  .row {
    display: grid;
    grid-template-columns: subgrid;
    align-items: center;
    gap: var(--space-md);
    padding: 10px 0;
    border-top: 1px solid var(--background-lighter);
  }
  /* checkbox rows lay out on their own, so a long toggle label does not
     widen the label column and squeeze the fields */
  .row:has(> input[type='checkbox']) {
    grid-template-columns: minmax(0, 1fr) auto;
  }
  .row:first-of-type {
    border-top: none;
  }
  input[type='checkbox'] {
    justify-self: end;
    width: 20px;
    height: 20px;
    accent-color: var(--accent);
  }
  .path {
    display: flex;
    gap: 8px;
  }
  .path input {
    flex: 1;
    min-width: 0;
  }
  .seg {
    justify-self: end;
    display: inline-flex;
    background: var(--background-darker);
    border-radius: 8px;
    padding: 3px;
  }
  .seg button {
    padding: 6px 14px;
    border-radius: 6px;
    color: var(--text-secondary);
  }
  .seg button.on {
    background: var(--accent);
    color: var(--accent-text);
  }
  .swatches {
    justify-self: end;
    display: flex;
    align-items: center;
    gap: 10px;
    position: relative;
  }
  .swatch {
    position: relative;
    width: 26px;
    height: 26px;
    padding: 0;
    border-radius: 50%;
    border: 2px solid color-mix(in srgb, var(--text-default) 22%, transparent);
    background: var(--c);
    background-clip: padding-box;
    transition: transform 0.12s ease, box-shadow 0.15s ease;
  }
  .swatch:hover {
    transform: scale(1.12);
  }
  .swatch.on {
    border-color: var(--background);
    box-shadow: 0 0 0 2px var(--c);
  }
  .swatch.custom:not(.on) {
    /* background-image keeps background-clip: padding-box, so the ring matches the others */
    background-image: conic-gradient(#ff5c5c, #f0c419, #22b573, #2fa8e8, #8f7dff, #ff5c9d, #ff5c5c);
  }
  .color-input {
    position: absolute;
    right: 0;
    bottom: 0;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
  h3 {
    margin: 14px 0 0;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.8px;
    text-transform: uppercase;
    color: var(--text-tertiary);
  }
  h3 + .row {
    border-top: none;
  }
  .path-link {
    padding: 0;
    text-align: left;
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    font-size: 12px;
    color: var(--text-secondary);
    word-break: break-all;
    text-decoration: underline;
    text-decoration-color: transparent;
    text-underline-offset: 3px;
    transition: color var(--transition), text-decoration-color var(--transition);
  }
  .path-link:hover,
  .path-link:focus-visible {
    color: var(--accent-overlay);
    text-decoration-color: currentColor;
  }
  .about-actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--background-lighter);
  }
  /* monochrome: no accent, just text colour and a hairline */
  .btn.mono {
    justify-content: center;
    background: transparent;
    border: 1px solid color-mix(in srgb, var(--text-default) 28%, transparent);
    color: var(--text-default);
  }
  .btn.mono:hover {
    background: color-mix(in srgb, var(--text-default) 8%, transparent);
  }
  .btn.mono svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.8;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .btn.danger {
    color: var(--danger);
  }
  code {
    font-size: 12px;
    color: var(--text-secondary);
    user-select: text;
    word-break: break-all;
  }
  .small {
    font-size: 12px;
  }
  .hint {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-secondary);
  }
</style>
