<script lang="ts">
  // Generated cover for a game without art (games added by hand): the
  // executable's icon over a blurred, enlarged copy of itself on an accent
  // gradient, with the title below. Fills its (3:4) parent; sizes follow the
  // card width through container units, so one look works for the small
  // desktop tiles and the big console ones.
  interface Props {
    title: string
    /** icon URL (asset:// or blob:), optional */
    icon?: string
  }
  let { title, icon = '' }: Props = $props()

  let broken = $state(false)
  $effect(() => {
    void icon
    broken = false
  })
  const showIcon = $derived(!!icon && !broken)
</script>

<div class="auto" aria-hidden="true">
  {#if showIcon}
    <img class="glow" src={icon} alt="" />
  {/if}
  <div class="shade"></div>
  <div class="content">
    {#if showIcon}
      <img class="icon" src={icon} alt="" onerror={() => (broken = true)} />
    {:else}
      <span class="letter">{title.trim()[0]?.toUpperCase() ?? '?'}</span>
    {/if}
    <span class="title">{title}</span>
  </div>
</div>

<style>
  .auto {
    position: absolute;
    inset: 0;
    overflow: hidden;
    container-type: inline-size;
    background:
      radial-gradient(120% 80% at 50% 30%, color-mix(in srgb, var(--accent) 55%, transparent), transparent 70%),
      linear-gradient(160deg, color-mix(in srgb, var(--accent) 35%, #0d0f14), #0d0f14);
  }
  .glow {
    position: absolute;
    left: 50%;
    top: 38%;
    width: 140%;
    height: auto;
    transform: translate(-50%, -50%);
    filter: blur(28px) saturate(1.4);
    opacity: 0.55;
  }
  .shade {
    position: absolute;
    inset: 0;
    background: linear-gradient(to bottom, transparent 45%, rgba(0, 0, 0, 0.55));
  }
  .content {
    position: relative;
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8cqw;
    padding: 10cqw 8cqw;
  }
  .icon {
    width: 46cqw;
    height: 46cqw;
    object-fit: contain;
    filter: drop-shadow(0 3cqw 6cqw rgba(0, 0, 0, 0.5));
  }
  .letter {
    font-size: 34cqw;
    line-height: 1;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.9);
    text-shadow: 0 3cqw 8cqw rgba(0, 0, 0, 0.45);
  }
  .title {
    max-width: 100%;
    font-size: 8.5cqw;
    font-weight: 600;
    line-height: 1.2;
    text-align: center;
    color: #fff;
    text-shadow: 0 1px 6px rgba(0, 0, 0, 0.6);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
  }
</style>
