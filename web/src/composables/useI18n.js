import { ref, computed } from 'vue'
import zh from '../i18n/zh'
import en from '../i18n/en'

const locales = { zh, en }
const locale = ref(localStorage.getItem('lobster-lang') || 'zh')

function setLocale(lang) {
  locale.value = lang
  localStorage.setItem('lobster-lang', lang)
}

function t(key) {
  const keys = key.split('.')
  let val = locales[locale.value]
  for (const k of keys) {
    val = val?.[k]
  }
  return val || key
}

export function useI18n() {
  return {
    locale,
    t,
    setLocale,
    toggleLocale: () => setLocale(locale.value === 'zh' ? 'en' : 'zh'),
    isZh: computed(() => locale.value === 'zh'),
  }
}
