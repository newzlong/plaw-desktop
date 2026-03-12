<template>
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{{ t('logs.title') }}</h1>
      </div>
      <div class="flex items-center gap-3">
        <GlassInput
          v-model="keyword"
          :placeholder="t('logs.searchPlaceholder')"
          style="min-width: 160px"
        />
        <GlassSelect
          v-model="level"
          :options="levelOptions"
          :placeholder="t('logs.allLevels')"
          class="w-36"
        />
      </div>
    </div>

    <GlassCard :hoverable="false">
      <div class="log-container">
        <div v-if="!logs.length" class="log-empty">
          {{ t('logs.noLogs') }}
        </div>
        <div
          v-for="(log, i) in filteredLogs"
          :key="i"
          class="log-line"
        >
          <span class="log-time">{{ log.timestamp || '--' }}</span>
          <span class="log-level" :class="`log-level--${(log.level || 'info').toLowerCase()}`">
            {{ log.level || 'INFO' }}
          </span>
          <span class="log-msg">{{ log.message }}</span>
        </div>
      </div>
    </GlassCard>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { GlassCard, GlassInput, GlassSelect } from '../components/glass'
import { getRecentLogs } from '../api/tauri'
import { useI18n } from '../composables/useI18n'

const { t } = useI18n()

const logs = ref([])
const keyword = ref('')
const level = ref('')
let timer = null

const levelOptions = computed(() => [
  { label: t('logs.all'), value: '' },
  { label: 'INFO', value: 'INFO' },
  { label: 'WARN', value: 'WARN' },
  { label: 'ERROR', value: 'ERROR' },
  { label: 'DEBUG', value: 'DEBUG' },
])

const filteredLogs = computed(() => {
  return logs.value.filter(l => {
    if (level.value && l.level !== level.value) return false
    if (keyword.value && !l.message?.includes(keyword.value)) return false
    return true
  })
})

async function refresh() {
  try {
    logs.value = await getRecentLogs(200, level.value || null, null)
  } catch { /* not ready */ }
}

onMounted(() => {
  refresh()
  timer = setInterval(refresh, 2000)
})
onUnmounted(() => clearInterval(timer))
</script>

<style scoped>
.page-header {
  display: flex; align-items: flex-start; justify-content: space-between;
  margin-bottom: 24px;
}
.page-title {
  font-size: 1.5rem; font-weight: 700;
  color: var(--text-primary);
  letter-spacing: -0.02em;
}
.log-container {
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.78rem;
}
.log-empty {
  color: var(--text-muted);
  font-size: 0.85rem;
  text-align: center;
  padding: 40px 0;
}
.log-line {
  display: flex; gap: 0.5rem; padding: 4px 0;
  border-bottom: 1px solid var(--border-subtle);
}
.log-time { color: var(--text-muted); min-width: 80px; }
.log-level { min-width: 44px; font-weight: 600; }
.log-level--info { color: var(--status-info); }
.log-level--warn { color: var(--status-warn); }
.log-level--error { color: var(--status-err); }
.log-level--debug { color: #A78BFA; }
.log-msg { color: var(--text-primary); word-break: break-all; }
</style>
