<template>
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{{ t('cron.title') }}</h1>
        <p class="page-desc">{{ t('cron.desc') }}</p>
      </div>
      <div class="flex items-center gap-2">
        <GlassButton size="sm" variant="ghost" @click="loadJobs">
          <RefreshCw class="w-4 h-4" />
        </GlassButton>
        <GlassButton size="sm" variant="primary" @click="openCreate">
          <Plus class="w-4 h-4" />
          {{ t('cron.newTask') }}
        </GlassButton>
      </div>
    </div>

    <!-- Not running -->
    <GlassCard v-if="!isRunning" :hoverable="false">
      <div class="empty-hint" style="color: var(--status-warn, #f59e0b)">
        <AlertCircle class="w-5 h-5" />
        <span>{{ t('cron.notRunning') }}</span>
      </div>
    </GlassCard>

    <!-- Loading -->
    <GlassCard v-else-if="loading" :hoverable="false">
      <div class="empty-hint">
        <Loader2 class="w-5 h-5 animate-spin" style="color: var(--text-muted)" />
        <span>{{ t('common.loading') || 'Loading...' }}</span>
      </div>
    </GlassCard>

    <!-- Error -->
    <GlassCard v-else-if="error" :hoverable="false">
      <div class="empty-hint" style="color: var(--status-err)">
        <AlertCircle class="w-5 h-5" />
        <span>{{ error }}</span>
        <GlassButton size="sm" variant="ghost" @click="loadJobs" class="ml-2">
          {{ t('common.retry') || 'Retry' }}
        </GlassButton>
      </div>
    </GlassCard>

    <!-- Empty state -->
    <GlassCard v-else-if="!jobs.length" :hoverable="false">
      <div class="empty-hint">
        <Clock class="w-5 h-5" style="color: var(--text-muted)" />
        <span>{{ t('cron.empty') }}</span>
      </div>
    </GlassCard>

    <!-- Job list -->
    <div v-else class="space-y-3">
      <GlassCard v-for="job in jobs" :key="job.id" :hoverable="false">
        <div class="task-row">
          <div class="task-info">
            <div class="task-header">
              <span class="task-name">{{ job.name || 'Unnamed Task' }}</span>
              <span v-if="job.enabled" class="status-badge status-ok">ON</span>
              <span v-else class="status-badge status-off">OFF</span>
            </div>
            <div class="task-meta">
              <span v-if="job.next_run" class="task-next">
                <CalendarClock class="w-3 h-3" />
                {{ formatTime(job.next_run) }}
              </span>
              <span v-if="job.last_status" class="task-last-status" :class="job.last_status === 'success' ? 'text-ok' : 'text-err'">
                {{ job.last_status }}
              </span>
            </div>
          </div>
          <div class="task-actions">
            <GlassButton size="sm" variant="ghost" class="delete-btn" @click="confirmDelete(job)">
              <Trash2 class="w-3.5 h-3.5" />
            </GlassButton>
          </div>
        </div>
        <!-- Session binding -->
        <div class="task-session">
          <template v-if="isSessionMissing(job.plaw_session)">
            <span class="session-warning">
              <AlertTriangle class="w-3.5 h-3.5" />
              {{ t('cron.sessionDeleted') || '关联会话已删除' }}
            </span>
            <select class="session-select" @change="rebindSession(job.id, $event.target.value)">
              <option value="" disabled selected>{{ t('cron.rebindSession') || '重新关联...' }}</option>
              <option v-for="opt in sessionOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
            </select>
          </template>
          <template v-else>
            <span class="session-label">{{ sessionName(job.plaw_session) }}</span>
            <select class="session-select session-select--inline" :value="job.plaw_session || ''" @change="rebindSession(job.id, $event.target.value)">
              <option v-for="opt in sessionOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
            </select>
          </template>
        </div>
        <div v-if="job.command" class="task-command">
          {{ job.command }}
        </div>
        <div v-if="job.last_run" class="task-last-run">
          {{ t('cron.lastRun') || 'Last run' }}: {{ formatTime(job.last_run) }}
        </div>
      </GlassCard>
    </div>

    <!-- Create dialog -->
    <GlassDialog v-model="showForm" :title="t('cron.createTask')">
      <div class="dialog-form">
        <GlassInput v-model="form.name" :label="t('cron.taskName')" placeholder="daily-report" />
        <div>
          <label class="field-label">{{ t('cron.schedule') }}</label>
          <GlassSelect v-model="form.preset" :options="cronPresets" />
          <GlassInput
            v-model="form.schedule"
            placeholder="*/5 * * * *"
            class="mt-2"
            :hint="cronHuman(form.schedule)"
          />
        </div>
        <div>
          <label class="field-label">{{ t('cron.command') }}</label>
          <textarea
            v-model="form.command"
            class="glass-textarea"
            rows="3"
            placeholder="Send me a daily weather summary"
          />
        </div>
      </div>
      <template #footer>
        <GlassButton variant="ghost" @click="showForm = false">{{ t('common.cancel') }}</GlassButton>
        <GlassButton variant="primary" :disabled="saving" @click="createJob">
          {{ saving ? '...' : t('common.create') }}
        </GlassButton>
      </template>
    </GlassDialog>

    <!-- Delete confirm -->
    <GlassDialog v-model="showDeleteConfirm" :title="t('cron.deleteTitle')">
      <p class="dialog-hint">
        {{ t('cron.deleteConfirm') }} <strong>{{ deleteTarget?.name || deleteTarget?.id }}</strong>?
      </p>
      <template #footer>
        <GlassButton variant="ghost" @click="showDeleteConfirm = false">{{ t('common.cancel') }}</GlassButton>
        <GlassButton variant="primary" class="delete-btn" @click="doDelete">{{ t('common.delete') }}</GlassButton>
      </template>
    </GlassDialog>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted } from 'vue'
import { Plus, Clock, Trash2, RefreshCw, Loader2, AlertCircle, AlertTriangle, CalendarClock } from 'lucide-vue-next'
import { GlassCard, GlassButton, GlassInput, GlassSelect, GlassDialog } from '../components/glass'
import { useI18n } from '../composables/useI18n'
import { usePlawState } from '../composables/usePlawState'
import { getCronJobs, addCronJob, deleteCronJob, patchCronJob } from '../api/gateway'
import { listSessions } from '../api/tauri'

const { t } = useI18n()
const { isRunning } = usePlawState()

const jobs = ref([])
const allSessions = ref([])
const loading = ref(false)
const error = ref('')
const showForm = ref(false)
const saving = ref(false)
const showDeleteConfirm = ref(false)
const deleteTarget = ref(null)

async function refreshSessions() {
  allSessions.value = await listSessions()
}

function sessionName(sessionId) {
  if (!sessionId) return t('cron.sessionAuto') || '自动'
  const s = allSessions.value.find(s => s.id === sessionId)
  return s ? (s.title || sessionId.slice(0, 10)) : null
}

function isSessionMissing(sessionId) {
  if (!sessionId) return false
  return !allSessions.value.find(s => s.id === sessionId)
}

const sessionOptions = computed(() => {
  const opts = [{ label: t('cron.sessionAuto') || '自动', value: '' }]
  for (const s of allSessions.value) {
    opts.push({ label: s.title || s.id.slice(0, 10), value: s.id })
  }
  return opts
})

async function rebindSession(jobId, newSessionId) {
  try {
    await patchCronJob(jobId, { plaw_session: newSessionId || null })
    await loadJobs()
  } catch (e) {
    error.value = String(e.message || e)
  }
}

const form = ref({
  name: '', schedule: '0 9 * * *', command: '', preset: '0 9 * * *',
})

const cronPresets = computed(() => [
  { label: t('cron.every5min'), value: '*/5 * * * *' },
  { label: t('cron.everyHour'), value: '0 * * * *' },
  { label: t('cron.everyDay9'), value: '0 9 * * *' },
  { label: t('cron.everyMonday9'), value: '0 9 * * 1' },
  { label: t('cron.custom'), value: 'custom' },
])

watch(() => form.value.preset, (val) => {
  if (val !== 'custom') form.value.schedule = val
})

function cronHuman(expr) {
  if (!expr) return ''
  const map = {
    '*/5 * * * *': 'Every 5 minutes',
    '0 * * * *': 'Every hour',
    '0 9 * * *': 'Every day at 9:00',
    '0 9 * * 1': 'Every Monday at 9:00',
    '0 0 * * *': 'Every day at midnight',
  }
  return map[expr] || ''
}

function formatTime(iso) {
  if (!iso) return ''
  try {
    const d = new Date(iso)
    return d.toLocaleString()
  } catch {
    return iso
  }
}

async function loadJobs() {
  loading.value = true
  error.value = ''
  try {
    const [data] = await Promise.all([getCronJobs(), refreshSessions()])
    if (data && Array.isArray(data.jobs)) {
      jobs.value = data.jobs
    } else {
      jobs.value = []
    }
  } catch (e) {
    error.value = String(e.message || e)
  } finally {
    loading.value = false
  }
}

function openCreate() {
  form.value = { name: '', schedule: '0 9 * * *', command: '', preset: '0 9 * * *' }
  showForm.value = true
}

async function createJob() {
  const schedule = form.value.schedule.trim()
  const command = form.value.command.trim()
  if (!schedule || !command) return

  saving.value = true
  try {
    await addCronJob(form.value.name.trim() || null, schedule, command)
    showForm.value = false
    await loadJobs()
  } catch (e) {
    error.value = String(e.message || e)
  } finally {
    saving.value = false
  }
}

function confirmDelete(job) {
  deleteTarget.value = job
  showDeleteConfirm.value = true
}

async function doDelete() {
  if (!deleteTarget.value) return
  showDeleteConfirm.value = false
  try {
    await deleteCronJob(deleteTarget.value.id)
    await loadJobs()
  } catch (e) {
    error.value = String(e.message || e)
  } finally {
    deleteTarget.value = null
  }
}

// Auto-load when Plaw becomes running
watch(isRunning, (running) => {
  if (running) loadJobs()
})

onMounted(() => {
  if (isRunning.value) loadJobs()
})
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
.page-desc {
  color: var(--text-secondary);
  font-size: 0.875rem; margin-top: 4px;
}

.empty-hint {
  display: flex; align-items: center; gap: 10px;
  color: var(--text-secondary); font-size: 0.875rem;
}

.task-row {
  display: flex; align-items: flex-start; justify-content: space-between;
}
.task-header {
  display: flex; align-items: center; gap: 10px;
}
.task-name {
  font-size: 0.95rem; font-weight: 600;
  color: var(--text-primary);
}
.status-badge {
  font-size: 0.65rem; font-weight: 700;
  padding: 1px 6px; border-radius: 6px;
  text-transform: uppercase; letter-spacing: 0.05em;
}
.status-ok {
  color: var(--status-ok); background: rgba(52, 211, 153, 0.15);
}
.status-off {
  color: var(--text-muted); background: rgba(148, 163, 184, 0.15);
}
.task-meta {
  display: flex; gap: 12px; margin-top: 4px; align-items: center;
}
.task-next {
  display: flex; align-items: center; gap: 4px;
  font-size: 0.78rem; color: var(--text-secondary);
}
.task-last-status {
  font-size: 0.75rem; font-weight: 500;
}
.text-ok { color: var(--status-ok); }
.text-err { color: var(--status-err); }
.task-actions { display: flex; gap: 4px; }
.delete-btn { color: var(--status-err) !important; }

.task-command {
  margin-top: 8px; padding-top: 8px;
  border-top: 1px solid var(--border-subtle);
  font-size: 0.8rem; color: var(--text-secondary);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  white-space: pre-wrap;
}
.task-last-run {
  margin-top: 4px;
  font-size: 0.72rem; color: var(--text-muted);
}

.task-session {
  display: flex; align-items: center; gap: 8px;
  margin-top: 6px; font-size: 0.78rem;
}
.session-label {
  color: var(--text-secondary);
}
.session-warning {
  display: flex; align-items: center; gap: 4px;
  color: var(--status-warn, #f59e0b);
  font-weight: 500;
}
.session-select {
  padding: 2px 6px;
  background: var(--bg-raised);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: 0.75rem;
  cursor: pointer;
}
.session-select--inline {
  opacity: 0;
  transition: opacity var(--duration-fast);
}
.task-session:hover .session-select--inline {
  opacity: 1;
}
</style>
