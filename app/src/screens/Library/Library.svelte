<script lang="ts">
  import { gameStatus, install, launch, library, libraryStores, libraryTab, isInstalledOrInstalling } from '$lib/state/library.svelte'
  import { settings } from '$lib/state/settings.svelte'
  import { navigate } from '$lib/router.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { cardImage, imageSrc, prepareGames, runnerName } from '$lib/library/view'
  import ImportDialog from '$lib/components/ImportDialog.svelte'
  import AddGameDialog from '$lib/components/AddGameDialog.svelte'
  import AutoCover from '$lib/components/AutoCover.svelte'
  import Spinner from '$lib/components/Spinner.svelte'
  import { type GameInfo, type Runner } from '$types/index'

  // Y on a focused card (gamepad quick action): play when installed, else install.
  function quick(game: GameInfo) {
    if (game.isInstalled) void launch(game)
    else if (game.installable) void install(game)
  }

  type Filter = 'all' | 'installed' | Runner
  let wanted = $state<Filter>('all')
  let showImport = $state(false)
  let showAdd = $state(false)

  // Same rule as console mode: "All" only with two or more stores.
  const storeFilters = $derived(libraryStores())
  const showAll = $derived(storeFilters.length >= 2)
  // What the user picked is kept apart from what is shown: while that
  // choice is not available yet (the stores' games still loading at start),
  // a fallback shows, and the choice comes back by itself once it is.
  const filter = $derived<Filter>(libraryTab(wanted))
  let query = $state('')

  const games = $derived.by(() => {
    const q = query.trim().toLowerCase()
    const list = library.games
      .filter((g) => filter === 'all' || (filter === 'installed' ? isInstalledOrInstalling(g) : g.runner === filter))
      .filter((g) => !q || g.title.toLowerCase().includes(q))
    return prepareGames(list, settings.value, true, isInstalledOrInstalling)
  })
  const grayscale = $derived(settings.value.libraryGrayscaleUninstalled)

  // Images that failed to load fall back to the letter tile.
  let broken = $state<Record<string, true>>({})
</script>

<div class="library">
  <header>
    <h1>
      {t('library.title', 'Library')}
    </h1>
    <div class="tools">
      <input type="search" placeholder={t('console.search', 'Search')} bind:value={query} />
      <div class="seg">
        {#if showAll}
          <button class:on={filter === 'all'} onclick={() => (wanted = 'all')}>{t('library.all', 'All')}</button>
        {/if}
        {#each storeFilters as r (r)}
          <button class:on={filter === r} onclick={() => (wanted = r)}>{runnerName(r)}</button>
        {/each}
        <button class:on={filter === 'installed'} onclick={() => (wanted = 'installed')}>{t('library.installed', 'Installed')}</button>
      </div>
      <button class="btn" onclick={() => (showImport = true)} title={t('import.title', 'Import installed games')}>{t('import.button', 'Import')}</button>
      <button class="btn" onclick={() => (showAdd = true)} title={t('sideload.buttonHint', 'Add a game that is not from a store')}>{t('sideload.button', 'Add game')}</button>
    </div>
  </header>

  {#if library.loading || (games.length === 0 && library.refreshing.length)}
    <p class="muted loading"><Spinner size={28} />{t('console.loading', 'Loading your library…')}</p>
  {:else if games.length === 0}
    <p class="muted empty">{t('library.empty', 'No games yet. Log in to Epic Games or GOG to see your library, or add your own game.')}</p>
  {:else}
    <div class="grid">
      {#each games as game (`${game.runner}:${game.appName}`)}
        {@const key = `${game.runner}:${game.appName}`}
        {@const img = cardImage(game)}
        {@const s = gameStatus(game)}
        {@const downloading = s?.status === 'installing' || s?.status === 'updating'}
        {@const percent = downloading ? Math.max(0, Math.min(100, s?.progress?.percent ?? 0)) : 0}
        <button
          class="card"
          class:gray={grayscale && !game.isInstalled}
          class:downloading
          data-game-card
          data-installed={game.isInstalled}
          onclick={() => navigate(`/game/${game.runner}/${game.appName}`)}
          ongpquick={() => quick(game)}
        >
          <div class="art">
            {#if img && !broken[key]}
              <img src={img} alt="" loading="lazy" decoding="async" onerror={() => (broken[key] = true)} />
              {#if downloading}
                <!-- as in console mode: the colour cover fills in from the top over a
                     black-and-white copy as the download progresses -->
                <img class="colour" src={img} alt="" style:--p={`${percent}%`} />
              {/if}
            {:else if game.runner === 'sideload'}
              <AutoCover title={game.title} icon={imageSrc(game.artIcon)} />
            {:else}
              <span class="initial">{game.title[0]}</span>
            {/if}
            {#if downloading}
              <span class="percent">{Math.floor(percent)}%</span>
            {/if}
          </div>
          <div class="meta">
            <span class="title" title={game.title}>{game.title}</span>
            {#if game.isInstalled}<span class="installed" title={t('library.installedMark', 'Installed')} aria-label={t('library.installedMark', 'Installed')}></span>{/if}
          </div>
        </button>
      {/each}
    </div>
  {/if}
</div>

{#if showImport}
  <ImportDialog onclose={() => (showImport = false)} />
{/if}
{#if showAdd}
  <AddGameDialog onclose={() => (showAdd = false)} />
{/if}

<style>
  .library {
    padding: var(--space-lg) var(--space-xl);
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-md);
    margin-bottom: var(--space-lg);
  }
  h1 {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 0;
    font-size: 26px;
  }
  .loading {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .tools {
    display: flex;
    align-items: center;
    gap: var(--space-sm);
  }
  .seg {
    display: inline-flex;
    background: var(--background-darker);
    border-radius: 8px;
    padding: 3px;
  }
  .seg button {
    padding: 6px 12px;
    border-radius: 6px;
    color: var(--text-secondary);
  }
  .seg button.on {
    background: var(--background-lighter);
    color: var(--text-default);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: var(--space-md);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    text-align: left;
    border-radius: var(--card-radius);
    transition: transform var(--transition);
  }
  .card:hover {
    transform: translateY(-3px);
  }
  .art {
    position: relative;
    aspect-ratio: 3 / 4;
    border-radius: var(--card-radius);
    background: linear-gradient(160deg, var(--background-lighter), var(--background-darker));
    display: grid;
    place-items: center;
    overflow: hidden;
  }
  .art img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    transition: filter 0.25s ease;
  }
  /* Heroic-style: uninstalled games in black and white, colour on hover/focus */
  .card.gray .art img {
    filter: grayscale(1);
  }
  .card.gray:hover .art img,
  .card.gray:focus .art img {
    filter: grayscale(0);
  }
  /* installing: black-and-white underneath, the colour copy revealed down to --p */
  .card.downloading .art img:not(.colour) {
    filter: grayscale(1) brightness(0.55);
  }
  .colour {
    position: absolute;
    inset: 0;
    -webkit-mask-image: linear-gradient(to bottom, #000 calc(var(--p) - 6%), transparent calc(var(--p) + 6%));
    mask-image: linear-gradient(to bottom, #000 calc(var(--p) - 6%), transparent calc(var(--p) + 6%));
    transition: --p 900ms linear;
  }
  .percent {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 10px;
    text-align: center;
    font-size: 22px;
    font-weight: 300;
    letter-spacing: 1px;
    color: #fff;
    text-shadow: 0 2px 10px rgba(0, 0, 0, 0.8);
  }
  .initial {
    font-size: 48px;
    font-weight: 700;
    color: var(--text-tertiary);
  }
  .meta {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }
  .title {
    flex: 1;
    min-width: 0;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* installed: a green indicator next to the title */
  .installed {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--success);
    box-shadow: 0 0 6px color-mix(in srgb, var(--success) 60%, transparent);
    /* an empty box has no baseline: centre it on the title line */
    align-self: center;
  }
  .empty {
    padding: var(--space-xl) 0;
    text-align: center;
  }
</style>
