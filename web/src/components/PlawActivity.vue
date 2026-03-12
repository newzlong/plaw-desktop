<template>
  <div class="plaw-activity" :class="`plaw-activity--${mood}`">
    <div class="plaw-avatar" :class="{ 'plaw-avatar--bounce': isActive }">
      <span class="plaw-emoji">{{ emoji }}</span>
    </div>
    <div class="plaw-status">
      <div class="plaw-status__text">{{ statusText }}</div>
      <div v-if="detail" class="plaw-status__detail">{{ detail }}</div>
      <div class="plaw-status__mood">{{ moodLabel }}</div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { listenActivity, listenStatus } from '../api/events'
import { useI18n } from '../composables/useI18n'

const { t } = useI18n()

const props = defineProps({
  running: { type: Boolean, default: false },
  healthy: { type: Boolean, default: false },
})

// Current activity state
const activity = ref('idle')  // idle, thinking, tool, browsing, searching, coding, reading, sleeping, error
const toolName = ref('')
const lastActiveTime = ref(Date.now())
let idleTimer = null
let unlistenActivity = null
let unlistenStatus = null

// Map SSE events to plaw states
function onActivityEvent(ev) {
  lastActiveTime.value = Date.now()
  const type = ev.type || ''

  if (type === 'llm_request') {
    activity.value = 'thinking'
    toolName.value = ''
  } else if (type === 'tool_call_start') {
    const tool = ev.tool || ''
    toolName.value = tool
    if (tool.includes('web_search') || tool.includes('search')) {
      activity.value = 'searching'
    } else if (tool.includes('browser') || tool.includes('scrape')) {
      activity.value = 'browsing'
    } else if (tool.includes('write') || tool.includes('edit') || tool.includes('shell')) {
      activity.value = 'coding'
    } else if (tool.includes('read') || tool.includes('file')) {
      activity.value = 'reading'
    } else {
      activity.value = 'tool'
    }
  } else if (type === 'tool_call') {
    // Tool finished — back to thinking (LLM will process result)
    activity.value = 'thinking'
    toolName.value = ''
  } else if (type === 'agent_start') {
    activity.value = 'thinking'
    toolName.value = ''
  } else if (type === 'agent_end') {
    activity.value = 'idle'
    toolName.value = ''
  } else if (type === 'error') {
    activity.value = 'error'
    toolName.value = ev.message || ''
  }
}

// Check for idle/sleeping states
function checkIdle() {
  if (!props.running) {
    activity.value = 'sleeping'
    return
  }
  const elapsed = Date.now() - lastActiveTime.value
  if (activity.value !== 'idle' && activity.value !== 'sleeping') {
    if (elapsed > 30000) {
      activity.value = 'idle'
      toolName.value = ''
    }
  }
  if (activity.value === 'idle' && elapsed > 120000) {
    activity.value = 'sleeping'
  }
}

const emoji = computed(() => {
  const map = {
    sleeping: '\uD83E\uDD7A',  // pleading face (sleepy plaw)
    idle: '\uD83E\uDD9E',      // plaw
    thinking: '\uD83E\uDD14',  // thinking face
    searching: '\uD83D\uDD0D', // magnifying glass
    browsing: '\uD83C\uDF10',  // globe
    coding: '\u270D\uFE0F',    // writing hand
    reading: '\uD83D\uDCD6',   // open book
    tool: '\uD83D\uDD27',      // wrench
    error: '\uD83D\uDE35',     // dizzy face
  }
  return map[activity.value] || '\uD83E\uDD9E'
})

const mood = computed(() => {
  if (['thinking', 'searching', 'browsing', 'coding', 'reading', 'tool'].includes(activity.value)) return 'active'
  if (activity.value === 'error') return 'error'
  if (activity.value === 'sleeping') return 'sleeping'
  return 'idle'
})

const isActive = computed(() => mood.value === 'active')

const statusText = computed(() => {
  const map = {
    sleeping: t('plaw.sleeping'),
    idle: t('plaw.idle'),
    thinking: t('plaw.thinking'),
    searching: t('plaw.searching'),
    browsing: t('plaw.browsing'),
    coding: t('plaw.coding'),
    reading: t('plaw.reading'),
    tool: t('plaw.usingTool'),
    error: t('plaw.error'),
  }
  return map[activity.value] || t('plaw.idle')
})

const detail = computed(() => {
  if (toolName.value && ['tool', 'searching', 'browsing', 'coding', 'reading'].includes(activity.value)) {
    return toolName.value
  }
  if (activity.value === 'error' && toolName.value) {
    return toolName.value
  }
  return ''
})

const moodLabel = computed(() => {
  const map = {
    active: t('plaw.moodBusy'),
    error: t('plaw.moodConfused'),
    sleeping: t('plaw.moodSleeping'),
    idle: t('plaw.moodRelaxed'),
  }
  return map[mood.value] || ''
})

onMounted(async () => {
  if (!props.running) activity.value = 'sleeping'
  unlistenActivity = await listenActivity(onActivityEvent)
  idleTimer = setInterval(checkIdle, 5000)
})

onUnmounted(() => {
  unlistenActivity?.()
  unlistenStatus?.()
  clearInterval(idleTimer)
})
</script>

<style scoped>
.plaw-activity {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 20px 24px;
  border-radius: var(--radius-lg);
  border: 1px solid var(--border-subtle);
  background: var(--bg-surface);
  box-shadow: var(--shadow-card);
  transition: all var(--duration-normal) var(--ease-out);
}
.plaw-activity--active {
  border-color: rgba(59, 130, 246, 0.3);
  box-shadow: 0 0 20px rgba(59, 130, 246, 0.08);
}
.plaw-activity--error {
  border-color: rgba(239, 68, 68, 0.3);
}
.plaw-activity--sleeping {
  opacity: 0.7;
}

.plaw-avatar {
  width: 56px;
  height: 56px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  background: var(--bg-raised);
  font-size: 1.8rem;
  flex-shrink: 0;
  transition: transform var(--duration-normal) var(--ease-out);
}
.plaw-avatar--bounce {
  animation: plaw-bounce 2s ease-in-out infinite;
}

.plaw-status {
  flex: 1;
  min-width: 0;
}
.plaw-status__text {
  font-size: 1rem;
  font-weight: 600;
  color: var(--text-primary);
  letter-spacing: -0.01em;
}
.plaw-status__detail {
  font-size: 0.8rem;
  color: var(--text-secondary);
  margin-top: 2px;
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.plaw-status__mood {
  font-size: 0.72rem;
  color: var(--text-muted);
  margin-top: 4px;
}

@keyframes plaw-bounce {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-4px); }
}
</style>
