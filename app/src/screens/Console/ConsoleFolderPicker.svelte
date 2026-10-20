<script lang="ts">
  // Folder picker for console mode (the native dialog cannot be driven with
  // a controller). Places to jump to on top, the path as crumbs, drives with
  // their free space, then folders. Up/Down move, A opens, Y picks the folder
  // shown, X makes a new folder (on-screen keyboard), B goes up a level
  // (closes at the drives), LB/RB page, LT/RT first/last; Up from the first
  // row reaches the places, Left/Right move between them.
  import { tick } from 'svelte'
  import { createFolder, errorMessage, listFolderPlaces, listFolders } from '$lib/api/commands'
  import { gamepad, pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'
  import { keyboard, onKeyboardClosed, openKeyboard } from '$lib/state/keyboard.svelte'
  import { settings } from '$lib/state/settings.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { formatBytes } from '$lib/format'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'
  import { itemAt } from '$lib/dom'
  import type { FolderListing, PlaceEntry } from '$types/index'

  interface Props {
    title: string
    /** Folder to start in (its nearest existing parent when missing). */
    start: string
    onpick: (path: string) => void
    oncancel: () => void
  }
  let { title, start, onpick, oncancel }: Props = $props()

  const PAGE = 10

  let listing = $state<FolderListing | null>(null)
  let places = $state<PlaceEntry[]>([])
  let error = $state<string | null>(null)
  let index = $state(0)
  let placeIndex = $state(0)
  let zone = $state<'places' | 'list'>('list')
  let listEl = $state<HTMLElement | undefined>()
  // "New folder": the hidden input the on-screen keyboard types into
  let nameInput = $state<HTMLInputElement | undefined>()
  let naming = $state(false)
  let busy = $state(false)

  const atDrives = $derived(!!listing && !listing.path)
  const canPick = $derived(!!listing?.path && !busy)
  const rows = $derived(listing?.folders ?? [])
  const crumbs = $derived.by(() => {
    const p = listing?.path ?? ''
    if (!p) return []
    const parts = p.split(/[\\/]+/).filter(Boolean)
    // the drive keeps its separator ("C:\"), the rest are names
    return parts.map((name, i) => (i === 0 && /^[A-Za-z]:$/.test(name) ? `${name}\\` : name))
  })
  const placeLabel = (id: string) =>
    id === 'games'
      ? t('console.folder.placeGames', 'Games library')
      : id === 'home'
        ? t('console.folder.placeHome', 'Home')
        : id === 'documents'
          ? t('console.folder.placeDocuments', 'Documents')
          : id === 'desktop'
            ? t('console.folder.placeDesktop', 'Desktop')
            : id === 'downloads'
              ? t('console.folder.placeDownloads', 'Downloads')
              : id
  // the drives come first in the places row
  const placeChips = $derived([{ id: 'drives', path: '' }, ...places])

  async function open(path: string | null, focusPath?: string) {
    try {
      const l = await listFolders(path)
      listing = l
      error = null
      zone = 'list'
      const back = focusPath ? l.folders.findIndex((f) => f.path === focusPath) : -1
      index = Math.max(0, back)
    } catch (e) {
      error = errorMessage(e)
    }
  }

  $effect(() => {
    void open(start || null)
    listFolderPlaces()
      .then((p) => (places = p))
      .catch(() => (places = []))
  })

  $effect(() => {
    void index
    void listing
    if (zone === 'list') tick().then(() => itemAt(listEl, index)?.scrollIntoView({ block: 'nearest' }))
  })

  function up() {
    if (naming) return
    if (!listing || listing.parent === null) return oncancel()
    void open(listing.parent || null, listing.path)
  }

  function activate(i = index) {
    const row = rows[i]
    if (row) void open(row.path)
  }

  function goPlace(i = placeIndex) {
    const chip = placeChips[i]
    if (chip) void open(chip.path || null)
  }

  function pick() {
    if (canPick && listing) onpick(listing.path)
  }

  // New folder: the keyboard types into the hidden input; Done creates the
  // folder and opens it, B / Escape just closes
  function startNaming() {
    if (!canPick || !nameInput || naming) return
    naming = true
    nameInput.value = ''
    const previous = onKeyboardClosed((submitted) => {
      onKeyboardClosed(previous)
      naming = false
      const name = nameInput?.value.trim() ?? ''
      if (submitted && name) void create(name)
    })
    openKeyboard(nameInput, settings.value.consoleModeKeyboardLayout, t('console.folder.newFolderName', 'Name of the new folder'))
  }

  async function create(name: string) {
    if (!listing?.path) return
    busy = true
    try {
      const path = await createFolder(listing.path, name)
      await open(path)
    } catch (e) {
      error = errorMessage(e)
    } finally {
      busy = false
    }
  }

  $effect(() =>
    pushHandler((action) => {
      if (naming || keyboard.open) return false
      const dir = directionOf(action)
      if (zone === 'places') {
        if (dir === 'left') placeIndex = Math.max(0, placeIndex - 1)
        else if (dir === 'right') placeIndex = Math.min(placeChips.length - 1, placeIndex + 1)
        else if (dir === 'down') zone = 'list'
        else if (action === 'mainAction') goPlace()
        else if (action === 'back') up()
        else if (action === 'altAction') pick()
        else if (action === 'rightClick') startNaming()
        return true
      }
      if (dir === 'up') {
        if (index === 0) zone = 'places'
        else index -= 1
      } else if (dir === 'down') index = Math.min(rows.length - 1, index + 1)
      else if (action === 'prevPage') index = Math.max(0, index - PAGE)
      else if (action === 'nextPage') index = Math.min(rows.length - 1, index + PAGE)
      else if (action === 'leftTrigger') index = 0
      else if (action === 'rightTrigger') index = Math.max(0, rows.length - 1)
      else if (dir === 'left' || action === 'back') up()
      else if (dir === 'right' || action === 'mainAction') activate()
      else if (action === 'altAction') pick()
      else if (action === 'rightClick') startNaming()
      return true
    })
  )

  // keys the gamepad map has no action for
  function onKey(e: KeyboardEvent) {
    if (naming || keyboard.open) return
    if (e.key === 'Home') index = 0
    else if (e.key === 'End') index = Math.max(0, rows.length - 1)
    else if (e.key === 'n' || e.key === 'N') startNaming()
    else return
    e.preventDefault()
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="overlay" data-focus-scope>
  <div class="card">
    <header>
      <h2>{title}</h2>
      <div class="places" role="tablist">
        {#each placeChips as chip, i (chip.id)}
          <button
            class="chip"
            class:current={(chip.path || '') === (listing?.path || '')}
            class:focused={zone === 'places' && i === placeIndex}
            role="tab"
            aria-selected={(chip.path || '') === (listing?.path || '')}
            onmouseenter={() => {
              zone = 'places'
              placeIndex = i
            }}
            onclick={() => {
              placeIndex = i
              goPlace(i)
            }}
          >
            {#if chip.id === 'drives'}
              <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="M3 15h18M7 12h.01M7 9h.01" /></svg>
              {t('console.folder.drives', 'Drives')}
            {:else}
              {placeLabel(chip.id)}
            {/if}
          </button>
        {/each}
      </div>
    </header>

    <div class="where">
      <p class="crumbs" aria-label={listing?.path || t('console.folder.drives', 'Drives')}>
        {#if crumbs.length === 0}
          <span class="crumb current">{t('console.folder.drives', 'Drives')}</span>
        {:else}
          {#each crumbs as c, i (i)}
            {#if i > 0}<span class="sep">›</span>{/if}
            <span class="crumb" class:current={i === crumbs.length - 1}>{c}</span>
          {/each}
        {/if}
      </p>
      {#if listing?.free != null}
        <span class="free">{t('console.folder.freeHere', '{{free}} free', { free: formatBytes(listing.free) })}</span>
      {/if}
    </div>
    {#if error}
      <p class="error">{error}</p>
    {/if}

    <div class="list" bind:this={listEl}>
      {#each rows as row, i (row.path)}
        <button
          class="row"
          class:drive={atDrives}
          class:focused={zone === 'list' && i === index}
          data-item
          onmouseenter={() => {
            zone = 'list'
            index = i
          }}
          onclick={() => {
            index = i
            activate(i)
          }}
        >
          {#if atDrives}
            <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="M3 15h18M7 12h.01M7 9h.01" /></svg>
          {:else}
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
          {/if}
          <span class="name">{row.name}</span>
          {#if atDrives && row.free != null && row.total != null}
            <span class="space">
              <span class="bar"><span class="fill" style:width={`${Math.min(100, Math.round((1 - row.free / Math.max(1, row.total)) * 100))}%`}></span></span>
              <span class="space-text">{t('console.folder.freeOf', '{{free}} free of {{total}}', { free: formatBytes(row.free), total: formatBytes(row.total) })}</span>
            </span>
          {/if}
        </button>
      {:else}
        {#if listing}<p class="empty">{t('console.folder.empty', 'No folders here')}</p>{/if}
      {/each}
    </div>

    <footer>
      <div class="actions">
        <button class="act primary" disabled={!canPick} onclick={pick}>
          {#if gamepad.connected}<ButtonGlyph button="y" layout={gamepad.layout} size={22} />{/if}
          {t('console.folder.useThis', 'Use this folder')}
        </button>
        <button class="act" disabled={!canPick} onclick={startNaming}>
          {#if gamepad.connected}<ButtonGlyph button="x" layout={gamepad.layout} size={22} />{:else}<kbd>N</kbd>{/if}
          {t('console.folder.newFolder', 'New folder')}
        </button>
      </div>
      <p class="hint-row">
        <span>{#if gamepad.connected}<ButtonGlyph button="main" layout={gamepad.layout} size={22} />{:else}<kbd>Enter</kbd>{/if} {t('console.folder.open', 'Open')}</span>
        <span>{#if gamepad.connected}<ButtonGlyph button="back" layout={gamepad.layout} size={22} />{:else}<kbd>Esc</kbd>{/if} {atDrives ? t('console.back', 'Back') : t('console.folder.up', 'Up')}</span>
        <span>{#if gamepad.connected}<ButtonGlyph button="l1" layout={gamepad.layout} size={22} /><ButtonGlyph button="r1" layout={gamepad.layout} size={22} />{:else}<kbd>PgUp</kbd><kbd>PgDn</kbd>{/if} {t('console.folder.page', 'Page')}</span>
        <span>{#if gamepad.connected}<ButtonGlyph button="l2" layout={gamepad.layout} size={22} /><ButtonGlyph button="r2" layout={gamepad.layout} size={22} />{:else}<kbd>Home</kbd><kbd>End</kbd>{/if} {t('console.folder.firstLast', 'Start / end')}</span>
      </p>
    </footer>

    <input class="sr" bind:this={nameInput} tabindex="-1" aria-hidden="true" />
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
    position: relative;
    width: min(960px, 90vw);
    height: min(720px, 86vh);
    display: flex;
    flex-direction: column;
    padding: 28px 32px 22px;
    border-radius: 20px;
    background: rgba(255, 255, 255, 0.06);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 24px;
    margin-bottom: 14px;
  }
  h2 {
    font-size: 26px;
    margin: 0;
    white-space: nowrap;
  }
  .places {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border-radius: 999px;
    font-size: 15px;
    color: var(--text-secondary);
    background: rgba(255, 255, 255, 0.06);
    outline: none;
    transition: box-shadow 120ms ease, background 120ms ease, color 120ms ease;
  }
  .chip svg {
    width: 18px;
    height: 18px;
  }
  .chip.current {
    color: var(--text-default);
    background: color-mix(in srgb, var(--accent-overlay) 35%, transparent);
  }
  .chip.focused {
    box-shadow: 0 0 0 3px var(--console-focus, #fff);
    color: var(--text-default);
  }
  .where {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
    margin: 0 0 12px;
    padding: 0 6px;
  }
  .crumbs {
    margin: 0;
    min-width: 0;
    font-size: 17px;
    color: var(--text-tertiary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .crumb.current {
    color: var(--text-default);
    font-weight: 600;
  }
  .sep {
    margin: 0 8px;
  }
  .free {
    flex: none;
    font-size: 15px;
    color: var(--text-secondary);
  }
  .error {
    color: var(--danger);
    margin: 0 0 12px;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    scrollbar-width: none;
    /* room for the focus ring: a scroll container clips box-shadow */
    padding: 4px;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 16px;
    min-height: 60px;
    padding: 8px 20px;
    margin-bottom: 6px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.04);
    font-size: 19px;
    color: var(--text-default);
    text-align: left;
    outline: none;
    transition: box-shadow 120ms ease, background 120ms ease;
  }
  .row.drive {
    min-height: 72px;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .space {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 6px;
    width: 260px;
  }
  .bar {
    width: 100%;
    height: 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    border-radius: 4px;
    background: var(--accent-overlay);
  }
  .space-text {
    font-size: 14px;
    color: var(--text-secondary);
  }
  .row.focused {
    background: rgba(255, 255, 255, 0.1);
    box-shadow: 0 0 0 3px var(--console-focus, #fff);
  }
  .row svg,
  .chip svg {
    flex: none;
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .row svg {
    width: 26px;
    height: 26px;
    color: var(--text-secondary);
  }
  .empty {
    color: var(--text-tertiary);
    font-size: 17px;
    padding: 12px 20px;
    margin: 0;
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    margin-top: 14px;
    padding-top: 14px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }
  .actions {
    display: flex;
    gap: 10px;
  }
  .act {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 10px 18px;
    border-radius: 12px;
    font-size: 16px;
    font-weight: 600;
    white-space: nowrap;
    color: var(--text-default);
    background: rgba(255, 255, 255, 0.08);
    transition: filter 120ms ease;
  }
  .act.primary {
    color: var(--accent-text);
    background: var(--accent);
  }
  .act:hover:not(:disabled) {
    filter: brightness(1.15);
  }
  .act:disabled {
    opacity: 0.4;
  }
  .hint-row {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 18px;
    margin: 0;
    font-size: 14px;
    color: var(--text-secondary);
  }
  .hint-row span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  kbd {
    padding: 2px 7px;
    border-radius: 6px;
    font: inherit;
    font-size: 12px;
    background: rgba(255, 255, 255, 0.1);
    color: var(--text-default);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
    pointer-events: none;
  }
</style>
