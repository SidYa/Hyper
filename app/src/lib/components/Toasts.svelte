<script lang="ts">
  import { toasts, dismiss } from '$lib/state/toast.svelte'
</script>

<div class="toasts">
  {#each toasts.list as t (t.id)}
    <button class="toast {t.kind}" onclick={() => dismiss(t.id)}>{t.message}</button>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 20px;
    bottom: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 1000;
    max-width: 420px;
  }
  .toast {
    text-align: left;
    padding: 12px 16px;
    border-radius: 10px;
    background: var(--background-lighter);
    color: var(--text-default);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.4);
    border-left: 4px solid var(--accent);
    user-select: text;
  }
  .toast.error {
    border-left-color: var(--danger);
  }
  .toast.success {
    border-left-color: var(--success);
  }
</style>
