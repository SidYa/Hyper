<script lang="ts">
  // Every entry shares one layout (the store rows were the reference):
  //   [16px marker] Title
  //                 subtitle
  import { router, navigate } from '$lib/router.svelte'
  import { t } from '$lib/i18n/index.svelte'
  import { library, storeOf } from '$lib/state/library.svelte'
  import { errorMessage, storeLoginStart } from '$lib/api/commands'
  import { notify } from '$lib/state/toast.svelte'
  import { RUNNER_NAMES, RUNNERS, type Runner } from '$types/index'
  import Brand from './Brand.svelte'

  const route = $derived(router.route)
  const queued = $derived(library.downloads.length)
  const owned = $derived(library.games.filter((g) => !g.isDlc))
  const installed = $derived(owned.filter((g) => g.isInstalled).length)

  let busy = $state<Runner | null>(null)

  // Click on a store: open its page when logged in, else start the login.
  // Logging out lives in Settings → Accounts.
  async function storeClick(runner: Runner) {
    const status = storeOf(runner)
    busy = runner
    try {
      if (status?.loggedIn) navigate(`/store/${runner}`)
      else await storeLoginStart(runner)
    } catch (e) {
      notify(errorMessage(e), 'error')
    } finally {
      busy = null
    }
  }
</script>

<nav class="sidebar">
  <div class="brand" data-tauri-drag-region><Brand size={21} /></div>

  <button class="entry item" class:active={route.name === 'library' || route.name === 'game'} onclick={() => navigate('/library')}>
    <svg class="mark" viewBox="0 0 24 24"><rect x="4" y="4" width="7" height="7" rx="1.5" /><rect x="13" y="4" width="7" height="7" rx="1.5" /><rect x="4" y="13" width="7" height="7" rx="1.5" /><rect x="13" y="13" width="7" height="7" rx="1.5" /></svg>
    <span class="title">{t('nav.library', 'Library')}</span>
    <span class="sub">{t('nav.librarySub', '{{count}} games · {{installed}} installed', { count: owned.length, installed })}</span>
  </button>

  {#each RUNNERS as runner (runner)}
    {@const status = storeOf(runner)}
    <button
      class="entry store"
      class:active={route.name === 'store' && route.params.runner === runner}
      disabled={busy === runner}
      title={status?.loggedIn ? RUNNER_NAMES[runner] : t('login.title', 'Log in')}
      onclick={() => storeClick(runner)}
    >
      <span class="mark dot-box"><span class="dot" class:on={status?.loggedIn}></span></span>
      <span class="title">{RUNNER_NAMES[runner]}</span>
      <span class="sub">
        {#if busy === runner}…{:else if status?.loggedIn}{status.username}{:else}{t('login.title', 'Log in')}{/if}
      </span>
    </button>
  {/each}

  <button class="entry item" class:active={route.name === 'downloads'} onclick={() => navigate('/downloads')}>
    <svg class="mark" viewBox="0 0 24 24"><path d="M12 4v11M7 10.5l5 5 5-5M5 20h14" /></svg>
    <span class="title">{t('nav.downloads', 'Downloads')}</span>
    <span class="sub">
      {queued > 0 ? t('nav.downloadsActive', '{{count}} in queue', { count: queued }) : t('nav.downloadsIdle', 'Nothing downloading')}
    </span>
    {#if queued > 0}<span class="badge">{queued}</span>{/if}
  </button>

  <button class="entry item" class:active={route.name === 'settings'} onclick={() => navigate('/settings')}>
    <svg class="mark" viewBox="0 0 24 24"><circle cx="12" cy="12" r="3" /><path d="M12 3v2.5M12 18.5V21M3 12h2.5M18.5 12H21M5.6 5.6l1.8 1.8M16.6 16.6l1.8 1.8M5.6 18.4l1.8-1.8M16.6 7.4l1.8-1.8" /></svg>
    <span class="title">{t('nav.settings', 'Settings')}</span>
    <span class="sub">{t('nav.settingsSub', 'Interface, accounts')}</span>
  </button>

  <div class="spacer"></div>

  <button class="item console" onclick={() => navigate('/console')}>
    <!-- Material Symbols Outlined "sports_esports" (Apache-2.0) -->
    <svg class="mark filled" viewBox="0 -960 960 960"><path d="M147-200q-38 0-59.5-25.5T72-296l48-335q8-53 49.5-91t94.5-38h433q53 0 94.5 38t49.5 91l47 335q6 45-15.5 70.5T813-200q-23 0-39-7.5T747-226L643-330H317L213-226q-11 11-27 18.5t-39 7.5Zm18-64 126-126h378l126 126q5 5 18 9 9 0 13.5-9t2.5-18l-48-339q-5-35-29.5-57T697-700H263q-30 0-54.5 22T179-621l-48 339q-2 9 2.5 18t13.5 9q7 0 18-9Zm563-188q12-12 12-28t-12-28q-12-12-28-12t-28 12q-12 12-12 28t12 28q12 12 28 12t28-12Zm-85-130q12-12 12-28t-12-28q-12-12-28-12t-28 12q-12 12-12 28t12 28q12 12 28 12t28-12ZM300-445h50v-75h75v-50h-75v-75h-50v75h-75v50h75v75Zm180-33Z" /></svg>
    <span>{t('nav.console', 'Console mode')}</span>
  </button>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    background: var(--navbar-background);
    padding: var(--space-md) var(--space-sm);
    gap: 2px;
    height: 100%;
  }
  /* wordmark centred across the sidebar */
  .brand {
    display: flex;
    justify-content: center;
    padding: 6px 10px 18px;
  }
  /* the wordmark is part of the window drag area (custom title bar) */
  .brand :global(*) {
    pointer-events: none;
  }

  /* shared row layout */
  .entry {
    display: grid;
    grid-template-columns: 16px 1fr auto;
    grid-template-rows: auto auto;
    column-gap: 12px;
    align-items: center;
    padding: 8px 12px;
    border-radius: 8px;
    text-align: left;
    color: var(--text-default);
    transition: background var(--transition);
  }
  .entry:hover {
    background: var(--background);
  }
  .entry.active {
    background: var(--current-background);
    box-shadow: inset 3px 0 0 var(--accent-overlay);
  }
  .entry:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .mark {
    grid-row: span 2;
    width: 16px;
    height: 16px;
    color: var(--text-secondary);
  }
  svg.mark {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.9;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  /* a filled icon (Material Symbols) among the stroked ones */
  svg.mark.filled {
    fill: currentColor;
    stroke: none;
  }
  .entry.active svg.mark {
    color: var(--accent-overlay);
  }
  .dot-box {
    display: grid;
    place-items: center;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-tertiary);
  }
  .dot.on {
    background: var(--success);
  }
  .title {
    grid-column: 2;
    line-height: 1.25;
  }
  .sub {
    grid-column: 2;
    font-size: 12px;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    grid-column: 3;
    grid-row: 1 / span 2;
    background: var(--accent);
    color: var(--accent-text);
    border-radius: 10px;
    padding: 0 7px;
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .console {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: 8px;
    border: 1px solid var(--background-lighter);
    color: var(--text-secondary);
    transition: background var(--transition), color var(--transition);
  }
  .console .mark {
    grid-row: auto;
    width: 20px;
    height: 20px;
    color: currentColor;
  }
  .console:hover {
    background: var(--background);
    color: var(--text-default);
  }
</style>
