<script lang="ts">
  // Wall clock for the console top bars (TV use: there is no taskbar).
  import { i18n } from '$lib/i18n/index.svelte'

  let now = $state(new Date())
  $effect(() => {
    const id = window.setInterval(() => (now = new Date()), 10_000)
    return () => window.clearInterval(id)
  })
  const text = $derived(now.toLocaleTimeString(i18n.locale, { hour: '2-digit', minute: '2-digit' }))
</script>

<span class="clock">{text}</span>

<style>
  .clock {
    font-size: 20px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
  }
</style>
