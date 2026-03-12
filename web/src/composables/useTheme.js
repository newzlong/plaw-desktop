import { ref, watchEffect } from 'vue'

const isDark = ref(localStorage.getItem('lobster-theme') !== 'light')

watchEffect(() => {
  document.documentElement.classList.toggle('dark', isDark.value)
  // Sync theme class to <body> so that <Teleport to="body"> content inherits CSS variables
  document.body.classList.toggle('app-dark', isDark.value)
  document.body.classList.toggle('app-light', !isDark.value)
  localStorage.setItem('lobster-theme', isDark.value ? 'dark' : 'light')
})

export function useTheme() {
  return {
    isDark,
    toggle: () => { isDark.value = !isDark.value },
  }
}
