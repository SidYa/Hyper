<script lang="ts">
  // Add a game that is not from a store (any executable: a game in
  // development, an old copy, an emulator…), or edit one added before.
  // Without a picture of its own the card is generated from the
  // executable's icon and the title (AutoCover).
  import { onMount, tick, untrack } from 'svelte'
  import { open } from '@tauri-apps/plugin-dialog'
  import { errorMessage, exeIcon, sideloadAdd, sideloadEdit, sideloadGet } from '$lib/api/commands'
  import { pushHandler } from '$lib/gamepad/input.svelte'
  import { notify } from '$lib/state/toast.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { imageSrc } from '$lib/library/view'
  import AutoCover from './AutoCover.svelte'
  import type { GameInfo } from '$types/index'

  interface Props {
    /** the sideloaded game to edit; omitted to add a new one */
    game?: GameInfo
    onclose: () => void
    onsaved?: (game: GameInfo) => void
  }
  let { game, onclose, onsaved }: Props = $props()

  // the form is seeded once from the game being edited
  const initial = untrack(() => game)
  const editing = !!initial
  const IMAGE_FILTER = [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif'] }]

  let title = $state(initial?.title ?? '')
  let executable = $state(initial?.install?.executable ?? '')
  let launchArgs = $state('')
  // null: unchanged, '': removed, path: new file
  let cover = $state<string | null>(null)
  let background = $state<string | null>(null)
  let coverBroken = $state(false)
  let iconUrl = $state(imageSrc(initial?.artIcon))
  let saving = $state(false)
  let firstInput = $state<HTMLInputElement | undefined>()
  // the title follows the file name until the user types one
  let titleTouched = editing

  const coverUrl = $derived(cover === null ? imageSrc(initial?.artSquare) : imageSrc(cover))
  const backgroundName = $derived(
    background === null ? (initial?.artBackground ? fileName(initial.artBackground) : '') : background ? fileName(background) : ''
  )
  const canSave = $derived(!!executable.trim() && !saving)

  function fileName(p: string) {
    return p.split(/[\\/]/).pop() ?? p
  }
  function stem(p: string) {
    return fileName(p).replace(/\.[^.]+$/, '')
  }

  let blobUrl = ''
  async function loadIcon(path: string) {
    const bytes = await exeIcon(path).catch(() => null)
    if (blobUrl) URL.revokeObjectURL(blobUrl)
    blobUrl = bytes ? URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: 'image/png' })) : ''
    iconUrl = blobUrl
  }

  async function pickExe() {
    const picked = await open({
      multiple: false,
      defaultPath: executable || undefined,
      filters: [
        { name: t('sideload.programs', 'Programs'), extensions: ['exe', 'bat', 'cmd'] },
        { name: t('sideload.allFiles', 'All files'), extensions: ['*'] }
      ]
    })
    if (typeof picked !== 'string') return
    executable = picked
    if (!titleTouched || !title.trim()) title = stem(picked)
    void loadIcon(picked)
  }

  async function pickImage(kind: 'cover' | 'background') {
    const picked = await open({ multiple: false, filters: IMAGE_FILTER })
    if (typeof picked !== 'string') return
    if (kind === 'cover') {
      cover = picked
      coverBroken = false
    } else background = picked
  }

  async function save() {
    if (!canSave) return
    saving = true
    const input = { title: title.trim(), executable: executable.trim(), launchArgs: launchArgs.trim(), cover, background }
    try {
      const saved = initial ? await sideloadEdit(initial.appName, input) : await sideloadAdd(input)
      notify(
        editing ? t('sideload.saved', '{{title}} saved', { title: saved.title }) : t('sideload.added', '{{title}} added to the library', { title: saved.title }),
        'success'
      )
      onsaved?.(saved)
      onclose()
    } catch (e) {
      notify(errorMessage(e), 'error', 8000)
    } finally {
      saving = false
    }
  }

  onMount(() => {
    if (initial) {
      sideloadGet(initial.appName)
        .then((g) => (launchArgs = g.launchArgs))
        .catch(() => {})
    }
    tick().then(() => firstInput?.focus())
    const off = pushHandler((a) => {
      if (a === 'back' && !saving) {
        onclose()
        return true
      }
      return false
    })
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !saving) {
        e.stopPropagation()
        onclose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      off()
      window.removeEventListener('keydown', onKey, true)
      if (blobUrl) URL.revokeObjectURL(blobUrl)
    }
  })
</script>

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && !saving && onclose()}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="add-title" data-focus-scope>
    <h2 id="add-title">{editing ? t('sideload.editTitle', 'Edit game') : t('sideload.title', 'Add a game')}</h2>
    {#if !editing}
      <p class="muted small">
        {t('sideload.intro', 'Any game that is not from Epic or GOG: pick its executable. Without a picture of its own the card is made from the program icon.')}
      </p>
    {/if}

    <div class="body">
      <div class="cover-col">
        <div class="preview">
          {#if coverUrl && !coverBroken}
            <img src={coverUrl} alt="" onerror={() => (coverBroken = true)} />
          {:else}
            <AutoCover title={title.trim() || t('sideload.untitled', 'New game')} icon={iconUrl} />
          {/if}
        </div>
        <button class="btn" type="button" onclick={() => pickImage('cover')}>{t('sideload.pickCover', 'Choose cover…')}</button>
        {#if coverUrl}
          <button class="linkish" type="button" onclick={() => (cover = '')}>{t('sideload.autoCover', 'Use the generated cover')}</button>
        {/if}
        <span class="hint">{t('sideload.coverHint', 'Portrait, 3:4 (for example 600×800)')}</span>
      </div>

      <div class="fields">
        <label class="field">
          <span>{t('sideload.executable', 'Executable')}</span>
          <div class="path">
            <input bind:this={firstInput} bind:value={executable} placeholder="C:\Games\MyGame\MyGame.exe" spellcheck="false" />
            <button class="btn" type="button" onclick={pickExe}>{t('game.browse', 'Browse…')}</button>
          </div>
        </label>
        <label class="field">
          <span>{t('sideload.name', 'Name')}</span>
          <input bind:value={title} oninput={() => (titleTouched = true)} placeholder={executable ? stem(executable) : ''} />
        </label>
        <label class="field">
          <span>{t('sideload.args', 'Launch arguments')} <em>{t('sideload.optional', 'optional')}</em></span>
          <input bind:value={launchArgs} placeholder="-windowed -dev" spellcheck="false" />
        </label>
        <div class="field">
          <span>{t('sideload.background', 'Background for the game page')} <em>{t('sideload.optional', 'optional')}</em></span>
          <div class="path">
            <span class="file" class:none={!backgroundName}>{backgroundName || t('sideload.none', 'not set')}</span>
            <button class="btn" type="button" onclick={() => pickImage('background')}>{t('game.browse', 'Browse…')}</button>
            {#if backgroundName}
              <button class="remove" type="button" onclick={() => (background = '')} aria-label={t('sideload.removeImage', 'Remove')} title={t('sideload.removeImage', 'Remove')}>✕</button>
            {/if}
          </div>
        </div>
      </div>
    </div>

    <div class="actions">
      <button class="btn primary" disabled={!canSave} onclick={save}>
        {#if saving}<span class="spinner"></span>{/if}
        {editing ? t('sideload.save', 'Save') : t('sideload.add', 'Add to library')}
      </button>
      <button class="btn" disabled={saving} onclick={onclose}>{t('game.cancel', 'Cancel')}</button>
    </div>
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
    overflow-y: auto;
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
  .body {
    display: grid;
    grid-template-columns: 190px 1fr;
    gap: var(--space-lg);
  }
  @media (max-width: 640px) {
    .body {
      grid-template-columns: 1fr;
    }
  }
  .cover-col {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
  }
  .preview {
    position: relative;
    aspect-ratio: 3 / 4;
    border-radius: var(--card-radius, 10px);
    overflow: hidden;
    background: var(--background-darker);
    box-shadow: 0 10px 30px var(--shadow);
  }
  .preview img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .hint {
    font-size: 12px;
    color: var(--text-tertiary);
    text-align: center;
  }
  .linkish {
    font-size: 13px;
    color: var(--accent);
  }
  .linkish:hover {
    text-decoration: underline;
  }
  .fields {
    display: flex;
    flex-direction: column;
    gap: 14px;
    min-width: 0;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .field > span {
    font-size: 13px;
    color: var(--text-secondary);
  }
  em {
    font-style: normal;
    color: var(--text-tertiary);
  }
  .path {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .path input {
    flex: 1;
    min-width: 0;
  }
  .file {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
  }
  .file.none {
    color: var(--text-tertiary);
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
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    margin-right: 6px;
    vertical-align: -1px;
    border: 2px solid rgba(255, 255, 255, 0.35);
    border-top-color: #fff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
