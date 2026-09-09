// Keep keyboard focus within a modal and restore it when the modal closes.
export const vDialog = {
  mounted(el) {
    const previous = document.activeElement
    const controls = () =>
      [
        ...el.querySelectorAll(
          'button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary, [tabindex="0"]',
        ),
      ].filter((item) => item.getClientRects().length)
    const handleKey = (event) => {
      if (event.key !== 'Tab') return
      const items = controls()
      if (!items.length) {
        event.preventDefault()
        el.focus()
        return
      }
      const first = items[0],
        last = items.at(-1)
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
    el.tabIndex = -1
    el.addEventListener('keydown', handleKey)
    const frame = requestAnimationFrame(() => (controls()[0] || el).focus())
    el._dialogCleanup = () => {
      cancelAnimationFrame(frame)
      el.removeEventListener('keydown', handleKey)
      if (previous?.isConnected) previous.focus()
    }
  },
  unmounted(el) {
    el._dialogCleanup?.()
  },
}
