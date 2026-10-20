<script lang="ts">
  // The app's confirm dialog (see askConfirm). Esc / B cancel, Enter / A on
  // the focused button. Destructive questions focus Cancel first.
  import { tick } from 'svelte'
  import { answerConfirm, confirmState } from '$lib/state/confirm.svelte'
  import { pushHandler } from '$lib/gamepad/input.svelte'
  import { directionOf } from '$lib/gamepad/actions'

  const req = $derived(confirmState.current)
  let confirmBtn = $state<HTMLButtonElement | undefined>()
  let cancelBtn = $state<HTMLButtonElement | undefined>()

  $effect(() => {
    if (!req) return
    const danger = req.danger
    tick().then(() => (danger ? cancelBtn : confirmBtn)?.focus())
    // modal: A / B and Left / Right act on the two buttons, nothing behind
    // the dialog takes input (console mode routes the controller here too)
    const off = pushHandler((a) => {
      if (a === 'back') {
        answerConfirm(false)
        return true
      }
      const dir = directionOf(a)
      if (dir === 'left' || dir === 'right') {
        ;(document.activeElement === confirmBtn ? cancelBtn : confirmBtn)?.focus()
      } else if (a === 'mainAction') {
        answerConfirm(document.activeElement !== cancelBtn)
      }
      return true
    })
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        answerConfirm(false)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      off()
      window.removeEventListener('keydown', onKey, true)
    }
  })
</script>

{#if req}
  <div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && answerConfirm(false)}>
    <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" data-focus-scope>
      <div class="badge" class:danger={req.danger} aria-hidden="true">
        {#if req.danger}
          <svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3" /></svg>
        {:else}
          <svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5h.01" /></svg>
        {/if}
      </div>
      <h2 id="confirm-title">{req.title}</h2>
      {#if req.subject}<p class="subject">{req.subject}</p>{/if}
      {#if req.message}<p class="message">{req.message}</p>{/if}
      <div class="actions">
        <button class="btn" bind:this={cancelBtn} onclick={() => answerConfirm(false)}>{req.cancelLabel}</button>
        <button class="btn" class:primary={!req.danger} class:danger={req.danger} bind:this={confirmBtn} onclick={() => answerConfirm(true)}>
          {req.confirmLabel}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 950;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--body-background) 70%, transparent);
    backdrop-filter: blur(4px);
    animation: fade 140ms ease-out;
  }
  .dialog {
    width: min(420px, 92vw);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: var(--space-lg);
    border-radius: 14px;
    background: var(--background);
    border: 1px solid var(--background-lighter);
    box-shadow: 0 24px 60px var(--shadow);
    text-align: center;
    animation: pop 160ms ease-out;
  }
  .badge {
    width: 52px;
    height: 52px;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    color: var(--accent);
    margin-bottom: 4px;
  }
  .badge.danger {
    background: color-mix(in srgb, var(--danger) 16%, transparent);
    color: var(--danger);
  }
  .badge svg {
    width: 26px;
    height: 26px;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.8;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  h2 {
    margin: 0;
    font-size: 19px;
  }
  .subject {
    margin: 0;
    font-weight: 600;
    color: var(--text-default);
    overflow-wrap: anywhere;
  }
  .message {
    margin: 0;
    font-size: 14px;
    color: var(--text-secondary);
    white-space: pre-line;
  }
  .actions {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    width: 100%;
    margin-top: 10px;
  }
  .actions .btn {
    justify-content: center;
  }
  .btn.danger {
    background: var(--danger);
    color: #fff;
  }
  .btn.danger:hover {
    background: color-mix(in srgb, var(--danger) 85%, #fff);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.96);
    }
  }
</style>
