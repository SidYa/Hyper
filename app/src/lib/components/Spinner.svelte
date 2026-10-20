<script lang="ts">
  // Looping ring for work without a known end (library refresh).
  let { size = 20 }: { size?: number } = $props()
</script>

<span class="spinner" style:--size="{size}px" aria-hidden="true"></span>

<style>
  .spinner {
    display: inline-block;
    flex: none;
    width: var(--size);
    height: var(--size);
    border: max(2px, calc(var(--size) / 12)) solid var(--background-lighter);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation-duration: 2.4s;
    }
  }
</style>
