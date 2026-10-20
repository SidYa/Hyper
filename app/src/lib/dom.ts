// DOM lookups for the console lists.

/** Element of item `i` in a keyed `{#each}` list, marked `data-item`.
 *  Read from the DOM on purpose: `bind:this={els[i]}` slots go stale when
 *  items move or disappear (an uninstalled game sorted to the end kept its
 *  card at the old index, and focus jumped there), while the DOM order
 *  always matches the list. */
export function itemAt<T extends HTMLElement = HTMLElement>(list: Element | undefined, i: number): T | undefined {
  return list?.querySelectorAll<T>('[data-item]')[i]
}

/** Every `data-item` element of a list, in order. */
export function items<T extends HTMLElement = HTMLElement>(list: Element | undefined): T[] {
  return list ? Array.from(list.querySelectorAll<T>('[data-item]')) : []
}
