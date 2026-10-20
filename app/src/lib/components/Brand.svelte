<script lang="ts">
  // Wordmark: "Hyper" on an accent tile with a thin ring just outside it.
  // Everything is sized in em, so `size` scales the whole mark (sidebar,
  // console top bar).
  interface Props {
    size?: number
  }
  let { size = 20 }: Props = $props()
</script>

<span class="brand" style:font-size={`${size}px`} role="img" aria-label="Hyper">
  <span class="tile accent-live" aria-hidden="true"><b>Hyper</b></span>
</span>

<style>
  .brand {
    display: inline-flex;
    align-items: center;
    line-height: 1;
    white-space: nowrap;
    user-select: none;
    /* room for the ring, which sits outside the tile */
    padding: 3px;
  }
  .tile {
    display: inline-flex;
    align-items: center;
    height: 2em;
    padding: 0 1.35em;
    border-radius: 0.4em;
    /* gradient, drift and ring: .accent-live (app.css) */
    transition: color var(--transition), outline-color var(--transition);
  }
  .tile b {
    font-weight: 700;
    letter-spacing: 0.01em;
    line-height: 1;
    /* centre the letters themselves (cap height to baseline), not the
       font's line box, which has extra room for descenders */
    text-box: trim-both cap alphabetic;
    /* optical centre: a hair above the geometric one */
    transform: translateY(-0.05em);
  }
  /* hover: a lightning strike, once. A bright streak sweeps across the tile
     while the tile flashes twice (glow layer + the letters). The sidebar
     turns off pointer events inside the mark (it is a window drag area),
     so the parent being hovered counts too. */
  .brand {
    position: relative;
  }
  .brand::before {
    content: '';
    position: absolute;
    inset: 3px;
    border-radius: 0.4em;
    pointer-events: none;
    opacity: 0;
    box-shadow: 0 0 0.9em 0.15em color-mix(in srgb, #fff 55%, var(--accent-overlay));
  }
  .tile {
    position: relative;
    overflow: hidden;
  }
  .tile::after {
    content: '';
    position: absolute;
    top: -40%;
    bottom: -40%;
    left: -45%;
    width: 30%;
    pointer-events: none;
    opacity: 0;
    background: linear-gradient(100deg, transparent 0%, rgba(255, 255, 255, 0) 30%, rgba(255, 255, 255, 0.9) 50%, rgba(255, 255, 255, 0) 70%, transparent 100%);
    transform: skewX(-22deg) translateX(0);
  }
  .brand:hover::before,
  :global(:hover) > .brand::before {
    animation: strike 520ms ease-out both;
  }
  .brand:hover .tile::after,
  :global(:hover) > .brand .tile::after {
    animation: bolt 520ms cubic-bezier(0.2, 0.8, 0.3, 1) both;
  }
  .brand:hover .tile b,
  :global(:hover) > .brand .tile b {
    animation: flash 520ms ease-out both;
  }
  @keyframes bolt {
    0% {
      transform: skewX(-22deg) translateX(0);
      opacity: 0;
    }
    12% {
      opacity: 1;
    }
    40% {
      opacity: 0.35;
    }
    52% {
      opacity: 1;
    }
    100% {
      transform: skewX(-22deg) translateX(560%);
      opacity: 0;
    }
  }
  @keyframes strike {
    0%,
    100% {
      opacity: 0;
    }
    12% {
      opacity: 1;
    }
    32% {
      opacity: 0.2;
    }
    48% {
      opacity: 0.9;
    }
  }
  @keyframes flash {
    0%,
    100% {
      filter: brightness(1);
      text-shadow: none;
    }
    12% {
      filter: brightness(1.5);
      text-shadow: 0 0 0.5em rgba(255, 255, 255, 0.9);
    }
    32% {
      filter: brightness(1.05);
      text-shadow: none;
    }
    48% {
      filter: brightness(1.6);
      text-shadow: 0 0 0.6em rgba(255, 255, 255, 1);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .brand::before,
    .tile::after,
    .tile b {
      animation: none !important;
    }
  }
</style>
