;(() => {
  try {
    const mode = localStorage.getItem('pingora-panel.color-mode')
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches
    const dark = mode === 'dark' || ((mode === null || mode === 'auto') && prefersDark)
    document.documentElement.classList.toggle('dark', dark)
  } catch {
    // Storage can be unavailable; the application applies the scheme later.
  }
})()
