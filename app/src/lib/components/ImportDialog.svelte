<script lang="ts">
  // Re-attach games that are already on disk (e.g. after reinstalling Windows):
  // collect one or more folders → we list the owned titles found in all of
  // them (Epic and GOG) as covers → import the selected ones in one go.
  import { onMount, tick } from 'svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { errorMessage, findImportableGames, importGame } from '$lib/api/commands'
  import { pushHandler } from '$lib/gamepad/input.svelte'
  import { findGame } from '$lib/state/library.svelte'
  import { settings } from '$lib/state/settings.svelte'
  import { notify } from '$lib/state/toast.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { cardImage, runnerName } from '$lib/library/view'
  import AutoCover from './AutoCover.svelte'
  import type { ImportCandidate } from '$types/index'

  interface Props {
    onclose: () => void
  }
  let { onclose }: Props = $props()

  // The folder list is remembered on this PC so the next import starts from it.
  const FOLDERS_KEY = 'hyper.importFolders'
  function loadFolders(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(FOLDERS_KEY) ?? '[]')
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []
    } catch {
      return []
    }
  }
  function saveFolders(list: string[]) {
    try {
      localStorage.setItem(FOLDERS_KEY, JSON.stringify(list))
    } catch {
      /* storage unavailable: the list just is not remembered */
    }
  }

  type Phase = 'pick' | 'scanning' | 'list' | 'importing' | 'done'
  let phase = $state<Phase>('pick')
  let folders = $state<string[]>(loadFolders())
  let found = $state<ImportCandidate[]>([])
  let checked = $state<Record<string, boolean>>({})
  let results = $state<Record<string, 'ok' | 'busy' | string>>({})
  let broken = $state<Record<string, true>>({})
  let firstBtn = $state<HTMLButtonElement | undefined>()

  const key = (c: ImportCandidate) => `${c.runner}:${c.appName}`
  const selected = $derived(found.filter((c) => checked[key(c)]))
  const allChecked = $derived(found.length > 0 && selected.length === found.length)
  const okCount = $derived(Object.values(results).filter((r) => r === 'ok').length)
  const locked = $derived(phase === 'importing')

  function cover(c: ImportCandidate): string {
    const g = findGame(c.runner, c.appName)
    return g ? cardImage(g) : ''
  }

  async function addFolders() {
    const picked = await open({
      directory: true,
      multiple: true,
      defaultPath: folders.at(-1) || settings.value.defaultInstallPath || undefined
    })
    const list = Array.isArray(picked) ? picked : typeof picked === 'string' ? [picked] : []
    const norm = (p: string) => p.replace(/[\\/]+$/, '').toLowerCase()
    const next = [...folders]
    for (const p of list) if (!next.some((f) => norm(f) === norm(p))) next.push(p)
    folders = next
    saveFolders(next)
  }

  function removeFolder(i: number) {
    folders = folders.filter((_, j) => j !== i)
    saveFolders(folders)
  }

  function toggle(c: ImportCandidate) {
    if (phase !== 'list') return
    checked[key(c)] = !checked[key(c)]
  }
  function toggleAll() {
    const to = !allChecked
    checked = Object.fromEntries(found.map((c) => [key(c), to]))
  }

  async function scan() {
    if (!folders.length) return
    phase = 'scanning'
    results = {}
    try {
      found = await findImportableGames(folders)
      checked = Object.fromEntries(found.map((c) => [key(c), true]))
      phase = 'list'
      tick().then(() => firstBtn?.focus())
    } catch (e) {
      notify(errorMessage(e), 'error')
      phase = 'pick'
    }
  }

  async function run() {
    phase = 'importing'
    for (const c of selected) {
      results[key(c)] = 'busy'
      try {
        await importGame(c.runner, c.appName, c.path)
        results[key(c)] = 'ok'
      } catch (e) {
        results[key(c)] = errorMessage(e)
      }
    }
    phase = 'done'
    if (okCount > 0) notify(t('import.doneToast', '{{count}} games imported', { count: okCount }), 'success')
  }

  onMount(() => {
    tick().then(() => firstBtn?.focus())
    const off = pushHandler((a) => {
      if (a === 'back' && !locked) {
        onclose()
        return true
      }
      return false
    })
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !locked) {
        e.stopPropagation()
        onclose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      off()
      window.removeEventListener('keydown', onKey, true)
    }
  })
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && !locked && onclose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="imp-title" data-focus-scope>
    <h2 id="imp-title">{t('import.title', 'Import installed games')}</h2>
    <p class="muted small">
      {t('import.introMulti', 'Add the folder where your Epic Games or GOG games are installed, or pick several folders. The app scans them and imports every game that passes validation, that is, games actually licensed to your Epic Games or GOG account.')}
    </p>

    {#if phase === 'pick'}
      <div class="section">
        <span class="label">{t('import.foldersLabel', 'Game folders')}</span>
        <ul class="folders">
          {#each folders as f, i (f)}
            <li class="folder-row">
              <svg class="folder-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
              <span class="folder" title={f}>{f}</span>
              <button class="remove" onclick={() => removeFolder(i)} aria-label={t('import.remove', 'Remove folder')} title={t('import.remove', 'Remove folder')}>✕</button>
            </li>
          {/each}
          <li>
            <button class="add-folder" bind:this={firstBtn} onclick={addFolders}>
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 5v14M5 12h14" /></svg>
              {folders.length ? t('import.addMore', 'Add another folder…') : t('import.addFolders', 'Add folders…')}
            </button>
          </li>
        </ul>
      </div>
      <div class="actions">
        <button class="btn primary" disabled={!folders.length} onclick={scan}>{t('import.scan', 'Find games')}</button>
        <button class="btn" onclick={onclose}>{t('game.cancel', 'Cancel')}</button>
      </div>
    {:else if phase === 'scanning'}
      <div class="center">
        <span class="spinner big"></span>
        <span>{t('import.scanningMulti', 'Looking for games…')}</span>
      </div>
    {:else}
      {#if found.length === 0}
        <div class="center">
          <svg class="empty-icon" viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="6.5" /><path d="M16 16l4.5 4.5" /></svg>
          <span>{t('import.noneMulti', 'No games from your library were found in these folders.')}</span>
        </div>
      {:else}
        <div class="list-head">
          <span class="label">
            {t('import.foundCount', 'Found: {{count}}', { count: found.length })}
            {#if phase === 'list'}· {t('import.selectedCount', 'selected: {{count}}', { count: selected.length })}{/if}
          </span>
          {#if phase === 'list'}
            <button class="linkish" onclick={toggleAll}>
              {allChecked ? t('import.selectNone', 'Deselect all') : t('import.selectAll', 'Select all')}
            </button>
          {/if}
        </div>
        <div class="grid">
          {#each found as c (key(c))}
            {@const k = key(c)}
            {@const r = results[k]}
            {@const src = cover(c)}
            <button
              class="card"
              class:on={checked[k]}
              class:dim={phase !== 'list' && !checked[k]}
              aria-pressed={!!checked[k]}
              title={c.path}
              onclick={() => toggle(c)}
            >
              <span class="art">
                {#if src && !broken[k]}
                  <img src={src} alt="" loading="lazy" onerror={() => (broken[k] = true)} />
                {:else}
                  <AutoCover title={c.title} />
                {/if}
                {#if r === 'busy'}
                  <span class="overlay"><span class="spinner"></span></span>
                {:else if r === 'ok'}
                  <span class="overlay ok"><svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg></span>
                {:else if r}
                  <span class="overlay err" title={r}>!</span>
                {:else if phase === 'list'}
                  <span class="check" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M5 12.5l4.5 4.5L19 7.5" /></svg></span>
                {/if}
              </span>
              <span class="name">{c.title}</span>
              <span class="store">{runnerName(c.runner)}</span>
              {#if r && r !== 'ok' && r !== 'busy'}<span class="err-text">{r}</span>{/if}
            </button>
          {/each}
        </div>
      {/if}
      <div class="actions">
        {#if phase === 'list' && found.length > 0}
          <button class="btn primary" bind:this={firstBtn} disabled={selected.length === 0} onclick={run}>
            {t('import.run', 'Import {{count}}', { count: selected.length })}
          </button>
        {/if}
        {#if phase === 'importing'}
          <span class="status"><span class="spinner"></span>{t('import.running', 'Importing… this checks the game files and can take a minute.')}</span>
        {:else}
          {#if phase === 'done'}
            <button class="btn primary" bind:this={firstBtn} onclick={onclose}>{t('import.finish', 'Done')}</button>
          {/if}
          <button class="btn" onclick={() => (phase = 'pick')}>{t('import.folders', 'Folders…')}</button>
          {#if phase !== 'done'}
            <button class="btn" onclick={onclose}>{t('game.cancel', 'Cancel')}</button>
          {/if}
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 800;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--body-background) 70%, transparent);
    backdrop-filter: blur(4px);
  }
  .dialog {
    width: min(760px, 92vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: var(--space-lg);
    border-radius: 14px;
    background: var(--background);
    border: 1px solid var(--background-lighter);
    box-shadow: 0 24px 60px var(--shadow);
  }
  h2 {
    margin: 0;
    font-size: 20px;
  }
  .small {
    font-size: 13px;
    margin: 0;
  }
  .section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
  }
  .label {
    font-size: 13px;
    color: var(--text-secondary);
  }

  /* --- folders --- */
  .folders {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    overflow-y: auto;
    max-height: 46vh;
  }
  .folder-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 8px 8px 12px;
    border-radius: 10px;
    background: var(--background-darker);
    border: 1px solid var(--background-lighter);
  }
  .folder-icon {
    flex: none;
    width: 20px;
    height: 20px;
    fill: color-mix(in srgb, var(--accent) 25%, transparent);
    stroke: var(--accent);
    stroke-width: 1.6;
    stroke-linejoin: round;
  }
  .folder {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: ui-monospace, 'Cascadia Mono', Consolas, monospace;
    font-size: 13px;
  }
  .remove {
    flex: none;
    width: 28px;
    height: 28px;
    border-radius: 6px;
    color: var(--text-tertiary);
  }
  .remove:hover {
    background: var(--background-lighter);
    color: var(--danger);
  }
  .add-folder {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 12px;
    border-radius: 10px;
    border: 1.5px dashed var(--background-lighter);
    color: var(--text-secondary);
    font-size: 14px;
    transition: border-color 120ms ease, color 120ms ease, background 120ms ease;
  }
  .add-folder:hover,
  .add-folder:focus-visible {
    border-color: var(--accent);
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, transparent);
  }
  .add-folder svg {
    width: 16px;
    height: 16px;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
  }

  /* --- scanning / empty --- */
  .center {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: var(--space-xl) var(--space-lg);
    color: var(--text-secondary);
    text-align: center;
  }
  .empty-icon {
    width: 40px;
    height: 40px;
    fill: none;
    stroke: var(--text-tertiary);
    stroke-width: 1.6;
    stroke-linecap: round;
  }

  /* --- found games --- */
  .list-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
  }
  .linkish {
    font-size: 13px;
    color: var(--accent);
  }
  .linkish:hover {
    text-decoration: underline;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(118px, 1fr));
    gap: 14px;
    overflow-y: auto;
    min-height: 0;
    padding: 4px;
    margin: -4px;
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    text-align: left;
    border-radius: 12px;
    padding: 0;
    transition: opacity 150ms ease;
  }
  .card.dim {
    opacity: 0.4;
  }
  .art {
    position: relative;
    display: block;
    aspect-ratio: 3 / 4;
    border-radius: var(--card-radius, 10px);
    overflow: hidden;
    background: var(--background-darker);
    outline: 2px solid transparent;
    outline-offset: 2px;
    transition: outline-color 120ms ease, filter 150ms ease;
  }
  .art img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .card:not(.on) .art {
    filter: grayscale(0.85) brightness(0.7);
  }
  .card.on .art {
    outline-color: var(--accent);
  }
  .card:focus-visible .art {
    outline-color: var(--text-default);
  }
  .check {
    position: absolute;
    top: 6px;
    right: 6px;
    width: 22px;
    height: 22px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.45);
    border: 1.5px solid rgba(255, 255, 255, 0.7);
  }
  .card.on .check {
    background: var(--accent);
    border-color: var(--accent);
  }
  .check svg,
  .overlay svg {
    width: 14px;
    height: 14px;
    fill: none;
    stroke: #fff;
    stroke-width: 2.6;
    stroke-linecap: round;
    stroke-linejoin: round;
    opacity: 0;
  }
  .card.on .check svg,
  .overlay svg {
    opacity: 1;
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.5);
    color: #fff;
    font-size: 26px;
    font-weight: 700;
  }
  .overlay svg {
    width: 34px;
    height: 34px;
  }
  .overlay.ok {
    background: color-mix(in srgb, var(--success) 45%, rgba(0, 0, 0, 0.35));
  }
  .overlay.err {
    background: color-mix(in srgb, var(--danger) 55%, rgba(0, 0, 0, 0.35));
  }
  .name {
    font-size: 13px;
    font-weight: 500;
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .store {
    font-size: 11px;
    color: var(--text-tertiary);
  }
  .err-text {
    font-size: 11px;
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  /* --- footer --- */
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-secondary);
    font-size: 13px;
  }
  .spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    border: 2px solid var(--background-lighter);
    border-top-color: var(--accent-overlay);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .overlay .spinner {
    width: 26px;
    height: 26px;
    border-width: 3px;
    border-color: rgba(255, 255, 255, 0.3);
    border-top-color: #fff;
  }
  .spinner.big {
    width: 30px;
    height: 30px;
    border-width: 3px;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
