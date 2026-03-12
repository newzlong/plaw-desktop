<template>
  <div class="app-root" :class="isDark ? 'app-dark' : 'app-light'">
    <!-- Setup Wizard: full screen -->
    <router-view v-if="$route.path === '/setup'" />

    <!-- Main: Chat full screen -->
    <template v-else>
      <router-view />
    </template>

    <!-- Settings panel overlay -->
    <SettingsPanel ref="settingsPanelRef" v-model="showSettings" />

    <!-- Global toast notifications -->
    <NotificationToast />
  </div>
</template>

<script setup>
import { ref, provide, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { useTheme } from './composables/useTheme'
import { useI18n } from './composables/useI18n'
import { useNotifications } from './composables/useNotifications'
import NotificationToast from './components/NotificationToast.vue'
import SettingsPanel from './components/SettingsPanel.vue'
import { configExists, getAllUnconsumedNotifications, startPlaw } from './api/tauri'
import { usePlawState } from './composables/usePlawState'
import { listen } from '@tauri-apps/api/event'

const router = useRouter()
const { canStart } = usePlawState()
const { isDark, toggle } = useTheme()
const { t, toggleLocale, isZh } = useI18n()
const { addToast, setOnClick } = useNotifications()

// Settings panel state — provided to children so Chat.vue can open it
const showSettings = ref(false)
const settingsPanelRef = ref(null)

function openSettings(tabId) {
  if (tabId && settingsPanelRef.value) {
    settingsPanelRef.value.openTab(tabId)
  }
  showSettings.value = true
}

provide('showSettings', showSettings)
provide('openSettings', openSettings)
provide('toggleTheme', toggle)
provide('toggleLocale', toggleLocale)
provide('isDark', isDark)
provide('isZh', isZh)

// Toast click: navigate to the target session in chat
setOnClick((toast) => {
  if (toast.sessionId) {
    router.push({ path: '/', query: { session: toast.sessionId } })
  } else {
    router.push('/')
  }
})

// Listen for cron-result events from Tauri Rust SSE watcher
let unlistenCron = null
onMounted(async () => {
  try {
    const exists = await configExists()
    if (!exists && router.currentRoute.value.path !== '/setup') {
      router.replace('/setup')
    } else if (exists && canStart.value) {
      // Auto-start Plaw on app launch if config exists
      startPlaw().catch(() => {})
    }
  } catch { /* ignore */ }

  unlistenCron = await listen('cron-result', (event) => {
    const data = event.payload
    if (!data || data.type !== 'cron_result') return
    const jobName = data.job_name || 'cron'
    const status = data.status || 'unknown'
    const ok = status === 'ok'
    const icon = ok ? '\u2705' : '\u274C'
    const sessionId = data.lobster_session || null

    addToast({
      title: `${icon} ${jobName}`,
      body: ok ? (data.output || '\u4efb\u52a1\u5b8c\u6210') : (data.output || '\u4efb\u52a1\u5931\u8d25'),
      type: ok ? 'success' : 'error',
      sessionId,
      jobId: data.job_id,
      duration: 8000,
    })
  })

  document.addEventListener('visibilitychange', onVisibilityChange)
})

async function onVisibilityChange() {
  if (document.visibilityState !== 'visible') return
  try {
    const pending = await getAllUnconsumedNotifications()
    if (pending.length === 0) return
    for (const n of pending) {
      const ok = n.content.startsWith('\u2705')
      addToast({
        title: n.job_name || n.source || 'Lobster',
        body: n.content.split('\n').slice(0, 2).join(' '),
        type: ok ? 'success' : 'error',
        sessionId: n.session_id || null,
        jobId: n.job_id || null,
        duration: 10000,
      })
    }
  } catch {}
}

onUnmounted(() => {
  if (unlistenCron) unlistenCron()
  document.removeEventListener('visibilitychange', onVisibilityChange)
})
</script>

<style scoped>
.app-root {
  display: flex;
  height: 100vh;
  background: var(--bg-base);
  color: var(--text-primary);
  transition: background var(--duration-normal) var(--ease-out),
              color var(--duration-normal) var(--ease-out);
}
</style>
