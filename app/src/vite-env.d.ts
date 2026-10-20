/// <reference types="vite/client" />
/// <reference types="svelte" />

interface ImportMetaEnv {
  readonly VITE_DEVTOOLS?: string
}

// Custom DOM event fired by the gamepad layer on a focused game card (Y button).
declare module 'svelte/elements' {
  interface HTMLAttributes<T extends EventTarget> {
    ongpquick?: (event: CustomEvent & { currentTarget: EventTarget & T }) => void
  }
}

export {}
