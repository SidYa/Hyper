<script lang="ts">
  // Confirm dialog for console mode: A = confirm, B = cancel/dismiss.
  // Gamepad input is routed by the parent; this just renders.
  import { gamepad } from '$lib/gamepad/input.svelte'
  import ButtonGlyph from '$lib/components/ButtonGlyph.svelte'

  interface Props {
    title: string
    message?: string
    gameTitle?: string
    confirmLabel: string
    cancelLabel: string
    onconfirm: () => void
    oncancel: () => void
    danger?: boolean
  }
  let { title, message, gameTitle, confirmLabel, cancelLabel, onconfirm, oncancel, danger = false }: Props = $props()
</script>

<div class="overlay" data-focus-scope>
  <div class="card">
    <h2>{title}</h2>
    {#if gameTitle}<p class="game">{gameTitle}</p>{/if}
    {#if message}<p class="muted">{message}</p>{/if}
    {#if gamepad.connected}
      <p class="hint-row">
        <span><ButtonGlyph button="main" layout={gamepad.layout} size={26} /> {confirmLabel}</span>
        <span><ButtonGlyph button="back" layout={gamepad.layout} size={26} /> {cancelLabel}</span>
      </p>
    {/if}
    <div class="buttons">
      <button class="btn" class:primary={!danger} class:danger onclick={onconfirm} data-focus-initial>{confirmLabel}</button>
      <button class="btn" onclick={oncancel} data-dismiss>{cancelLabel}</button>
    </div>
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
    text-align: center;
    max-width: 640px;
    padding: 36px 48px;
    border-radius: 20px;
    background: rgba(255, 255, 255, 0.06);
  }
  h2 {
    font-size: 28px;
    margin: 0 0 8px;
  }
  .game {
    font-size: 18px;
    margin: 0 0 12px;
    color: var(--text-secondary);
  }
  .hint-row {
    display: flex;
    justify-content: center;
    gap: 28px;
    margin: 20px 0 10px;
  }
  .buttons {
    display: flex;
    justify-content: center;
    gap: 12px;
  }
  .btn.danger {
    background: var(--danger);
    color: #fff;
  }
  .hint-row span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  /* the quit warning lists what is busy, one line each */
  .card .muted {
    white-space: pre-line;
  }
</style>
