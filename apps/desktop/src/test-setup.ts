// jsdom lacks these browser APIs; Radix (tooltip, scroll area, menus) calls them.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
}
window.HTMLElement.prototype.scrollIntoView ??= () => {}
window.HTMLElement.prototype.hasPointerCapture ??= () => false
window.HTMLElement.prototype.setPointerCapture ??= () => {}
window.HTMLElement.prototype.releasePointerCapture ??= () => {}
Range.prototype.getBoundingClientRect ??= () => new DOMRect(0, 0, 0, 0)
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList
