<template>
  <div>
    <!-- Skeleton loading -->
    <template v-if="loading">
      <!-- Hero skeleton -->
      <div class="skel-hero">
        <div class="skel-hero__left">
          <GlassSkeleton circle height="40px" />
          <div class="skel-hero__text">
            <GlassSkeleton width="80px" height="1.1rem" />
            <GlassSkeleton width="140px" height="0.75rem" class="mt-1" />
          </div>
        </div>
        <div class="flex gap-2">
          <GlassSkeleton width="56px" height="30px" rounded />
          <GlassSkeleton width="56px" height="30px" rounded />
          <GlassSkeleton width="72px" height="30px" rounded />
        </div>
      </div>
      <!-- Stats skeleton -->
      <div class="stats-row">
        <div v-for="i in 4" :key="i" class="skel-stat">
          <GlassSkeleton circle height="40px" />
          <div class="flex flex-col gap-1.5" style="flex: 1">
            <GlassSkeleton width="60%" height="1.15rem" />
            <GlassSkeleton width="80%" height="0.75rem" />
          </div>
        </div>
      </div>
      <!-- Storage skeleton -->
      <div class="skel-storage mt-4">
        <GlassSkeleton width="220px" height="0.85rem" />
        <GlassSkeleton width="60px" height="30px" rounded />
      </div>
    </template>

    <!-- Real content -->
    <template v-else>
    <!-- Hero status -->
    <div class="hero-status" :class="isRunning ? 'hero-status--ok' : 'hero-status--off'">
      <div class="hero-status__left">
        <div class="hero-status__indicator">
          <span class="hero-status__dot" />
        </div>
        <div>
          <h1 class="hero-status__title">Plaw</h1>
          <p class="hero-status__subtitle">
            <template v-if="zcState === 'starting'">{{ t('dashboard.connecting') }}</template>
            <template v-else-if="zcState === 'stopping'">{{ t('common.stop') }}...</template>
            <template v-else-if="zcState === 'restarting'">{{ t('common.restart') }}...</template>
            <template v-else-if="zcState === 'crashed'">{{ t('dashboard.crashed') }}</template>
            <template v-else-if="isRunning">
              {{ t('dashboard.running') }}
              <span v-if="isHealthy" class="hero-status__health hero-status__health--ok">{{ t('dashboard.healthy') }}</span>
              <span v-else class="hero-status__health hero-status__health--wait">{{ t('dashboard.connecting') }}</span>
              <span v-if="zcPort" class="hero-status__port">:{{ zcPort }}</span>
              <span v-if="uptime" class="hero-status__uptime">{{ uptime }}</span>
            </template>
            <template v-else>{{ t('dashboard.stopped') }}</template>
          </p>
        </div>
      </div>
      <div class="flex gap-2">
        <GlassButton size="sm" variant="ghost" @click="openSettings('provider')">
          {{ t('common.config') }}
        </GlassButton>
        <GlassButton size="sm" variant="ghost" @click="openSettings('logs')">
          {{ t('nav.logs') }}
        </GlassButton>
        <template v-if="canStart">
          <GlassButton size="sm" variant="primary" :loading="zcState === 'starting'" @click="doStart">
            {{ t('common.start') }}
          </GlassButton>
        </template>
        <template v-else-if="canStop">
          <GlassButton size="sm" variant="ghost" :loading="zcState === 'stopping'" @click="doStop" class="stop-btn">
            {{ t('common.stop') }}
          </GlassButton>
          <GlassButton size="sm" variant="primary" :loading="zcState === 'restarting'" @click="doRestart">
            {{ t('common.restart') }}
          </GlassButton>
        </template>
        <template v-else>
          <!-- Busy state (starting/stopping/restarting) — buttons disabled via :loading -->
          <GlassButton size="sm" variant="ghost" :loading="true" disabled>
            {{ t('common.stop') }}
          </GlassButton>
        </template>
      </div>
    </div>

    <!-- Error message -->
    <div v-if="errorMsg" class="error-bar">{{ errorMsg }}</div>

    <!-- Stats row -->
    <div class="stats-row">
      <div class="stat-item">
        <div class="stat-item__icon">
          <BotIcon class="w-5 h-5" />
        </div>
        <div>
          <div class="stat-item__value">{{ provider || '--' }}</div>
          <div class="stat-item__label">{{ t('dashboard.provider') }}</div>
        </div>
      </div>

      <div class="stat-item">
        <div class="stat-item__icon stat-item__icon--amber">
          <RadioIcon class="w-5 h-5" />
        </div>
        <div>
          <div class="stat-item__value">{{ channelCount }}</div>
          <div class="stat-item__label">{{ t('dashboard.channels') }}</div>
        </div>
      </div>

      <div class="stat-item">
        <div class="stat-item__icon stat-item__icon--blue">
          <PuzzleIcon class="w-5 h-5" />
        </div>
        <div>
          <div class="stat-item__value">{{ skillCount }}</div>
          <div class="stat-item__label">{{ t('dashboard.skills') }}</div>
        </div>
      </div>

      <div class="stat-item">
        <div class="stat-item__icon stat-item__icon--green">
          <UsersIcon class="w-5 h-5" />
        </div>
        <div>
          <div class="stat-item__value">{{ agentCount }}</div>
          <div class="stat-item__label">{{ t('dashboard.agents') }}</div>
        </div>
      </div>
    </div>

    <!-- Storage management -->
    <div class="storage-row mt-4">
      <div class="storage-row__info">
        <span class="storage-row__label">{{ isZh ? '上传文件缓存' : 'Upload cache' }}</span>
        <span class="storage-row__detail">{{ uploadsFileCount }} {{ isZh ? '个文件' : 'files' }}, {{ formatSize(uploadsSize) }}</span>
      </div>
      <GlassButton size="sm" variant="ghost" :disabled="!uploadsFileCount || clearing" @click="doClearUploads">
        {{ clearing ? (isZh ? '清理中...' : 'Clearing...') : (isZh ? '清理' : 'Clear') }}
      </GlassButton>
    </div>

    <!-- Plaw activity -->
    <PlawActivity :running="isRunning" :healthy="isHealthy" class="mt-6" />

    <!-- Quick start (only when not configured) -->
    <GlassCard v-if="!provider || provider === '--'" :hoverable="false" class="mt-4">
      <div class="quick-start">
        <div>
          <h2 class="quick-start__title">{{ t('dashboard.getStarted') }}</h2>
          <p class="quick-start__desc">
            {{ t('dashboard.getStartedDesc') }}
          </p>
        </div>
        <GlassButton variant="primary" @click="openSettings('provider')">
          {{ t('dashboard.setupProvider') }}
        </GlassButton>
      </div>
    </GlassCard>
    </template><!-- end v-else -->
  </div>
</template>

<script setup>
import { ref, computed, inject, onMounted, onUnmounted, watch } from 'vue'
import { GlassCard, GlassButton, GlassSkeleton } from '../components/glass'
import PlawActivity from '../components/PlawActivity.vue'
import {
  Bot as BotIcon,
  Radio as RadioIcon,
  Puzzle as PuzzleIcon,
  Users as UsersIcon,
} from 'lucide-vue-next'
import { readConfig, startPlaw, stopPlaw, restartPlaw, getUploadsInfo, clearUploads } from '../api/tauri'
import { getSkills, resetPort } from '../api/gateway'
import { usePlawState } from '../composables/usePlawState'
import { useI18n } from '../composables/useI18n'

const { t, isZh } = useI18n()
const openSettings = inject('openSettings', () => {})
const { state: zcState, port: zcPort, startedAt, isRunning, isHealthy, isBusy, canStart, canStop } = usePlawState()

const loading = ref(true)
const provider = ref('')
const model = ref('')
const errorMsg = ref('')
const channelCount = ref(0)
const skillCount = ref(0)
const uploadsSize = ref(0)
const uploadsFileCount = ref(0)
const clearing = ref(false)
const agentCount = ref(0)
const nowSec = ref(Math.floor(Date.now() / 1000))
let uptimeTimer = null

const uptime = computed(() => {
  if (!startedAt.value || !isRunning.value) return ''
  const diff = nowSec.value - startedAt.value
  if (diff < 0) return ''
  if (diff < 60) return `${diff}s`
  if (diff < 3600) return `${Math.floor(diff / 60)}m ${diff % 60}s`
  const h = Math.floor(diff / 3600)
  const m = Math.floor((diff % 3600) / 60)
  return `${h}h ${m}m`
})

// Reverse-lookup table generalized from the old Kimi-special if-chain.
// Adding a new provider label = one entry. Lookup misses fall through to
// the generic `anthropic-custom:` strip OR the raw provider name.
const PROVIDER_LABELS = {
  // Exact match against `default_provider` on-disk values
  deepseek: 'DeepSeek',
  anthropic: 'Anthropic',
  openai: 'OpenAI',
  openrouter: 'OpenRouter',
  ollama: 'Ollama',
  gemini: 'Gemini',
  // anthropic-custom URLs (Kimi etc.) — match the on-disk form exactly
  'anthropic-custom:https://api.kimi.com/coding': 'Kimi Coder',
  'anthropic-custom:https://api.moonshot.cn': 'Kimi (Moonshot)',
}

function friendlyProvider(raw) {
  if (!raw) return '--'
  const direct = PROVIDER_LABELS[raw]
  if (direct) return direct
  if (raw.startsWith('anthropic-custom:')) return raw.replace('anthropic-custom:', '')
  return raw
}

async function loadConfigData() {
  try {
    const cfg = await readConfig()
    const rawProvider = cfg?.default_provider || ''
    provider.value = friendlyProvider(rawProvider)
    model.value = cfg?.default_model || ''
    let ch = 0
    if (cfg?.telegram?.bot_token) ch++
    if (cfg?.discord?.bot_token) ch++
    if (cfg?.slack?.bot_token) ch++
    channelCount.value = ch
    agentCount.value = cfg?.agents ? Object.keys(cfg.agents).length : 0
  } catch { /* config may not exist yet */ }
}

async function loadSkills() {
  if (!isRunning.value || !isHealthy.value) {
    skillCount.value = 0
    return
  }
  try {
    const skills = await getSkills()
    skillCount.value = Array.isArray(skills) ? skills.length : 0
  } catch { skillCount.value = 0 }
}

function formatSize(bytes) {
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
}

async function loadUploadsInfo() {
  try {
    const [size, count] = await getUploadsInfo()
    uploadsSize.value = size
    uploadsFileCount.value = count
  } catch { /* ignore */ }
}

async function doClearUploads() {
  clearing.value = true
  try {
    await clearUploads()
    uploadsSize.value = 0
    uploadsFileCount.value = 0
  } catch { /* ignore */ }
  clearing.value = false
}

// React to state changes from the global state machine
watch(zcState, (newState) => {
  if (newState === 'crashed') {
    errorMsg.value = t('dashboard.crashed')
    setTimeout(() => { errorMsg.value = '' }, 8000)
  }
  if (newState === 'healthy') {
    loadSkills()
  }
  if (newState === 'stopped' || newState === 'crashed') {
    skillCount.value = 0
  }
})

async function doStart() {
  errorMsg.value = ''
  try {
    await startPlaw()
    resetPort()
    await loadConfigData()
  } catch (e) {
    errorMsg.value = typeof e === 'string' ? e : (e?.message || t('dashboard.startFailed'))
    setTimeout(() => { errorMsg.value = '' }, 5000)
  }
}

async function doStop() {
  errorMsg.value = ''
  try {
    await stopPlaw()
  } catch (e) {
    errorMsg.value = typeof e === 'string' ? e : (e?.message || t('dashboard.stopFailed'))
    setTimeout(() => { errorMsg.value = '' }, 5000)
  }
}

async function doRestart() {
  errorMsg.value = ''
  try {
    await restartPlaw()
    resetPort()
    await loadConfigData()
  } catch (e) {
    errorMsg.value = typeof e === 'string' ? e : (e?.message || t('dashboard.restartFailed'))
    setTimeout(() => { errorMsg.value = '' }, 5000)
  }
}

onMounted(async () => {
  try {
    await Promise.all([loadConfigData(), loadUploadsInfo()])
    await loadSkills()
  } finally {
    loading.value = false
  }
  uptimeTimer = setInterval(() => { nowSec.value = Math.floor(Date.now() / 1000) }, 1000)
})
onUnmounted(() => {
  clearInterval(uptimeTimer)
})
</script>

<style scoped>
/* --- Skeleton --- */
.skel-hero {
  display: flex; align-items: center; justify-content: space-between;
  padding: 20px 24px;
  border-radius: var(--radius-lg);
  margin-bottom: 24px;
  border: 1px solid var(--border-subtle);
  background: var(--bg-surface);
  box-shadow: var(--shadow-card);
}
.skel-hero__left {
  display: flex; align-items: center; gap: 16px;
}
.skel-hero__text {
  display: flex; flex-direction: column; gap: 6px;
}
.skel-stat {
  display: flex; align-items: center; gap: 14px;
  padding: 18px 20px;
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-card);
}
.skel-storage {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 20px;
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-card);
}
.mt-1 { margin-top: 0.25rem; }

/* --- Hero Status Bar --- */
.hero-status {
  display: flex; align-items: center; justify-content: space-between;
  padding: 20px 24px;
  border-radius: var(--radius-lg);
  margin-bottom: 24px;
  border: 1px solid var(--border-subtle);
  background: var(--bg-surface);
  box-shadow: var(--shadow-card);
}
.hero-status--ok {
  border-left: 4px solid var(--status-ok);
}
.hero-status--off {
  border-left: 4px solid var(--status-err);
}

.hero-status__left {
  display: flex; align-items: center; gap: 16px;
}
.hero-status__indicator {
  width: 40px; height: 40px;
  display: flex; align-items: center; justify-content: center;
  border-radius: var(--radius-sm);
}
.hero-status--ok .hero-status__indicator {
  background: var(--status-ok-soft);
}
.hero-status--off .hero-status__indicator {
  background: var(--status-err-soft);
}
.hero-status__dot {
  width: 10px; height: 10px; border-radius: 50%;
}
.hero-status--ok .hero-status__dot {
  background: var(--status-ok);
  box-shadow: 0 0 8px rgba(34, 197, 94, 0.4);
}
.hero-status--off .hero-status__dot {
  background: var(--status-err);
}
.hero-status__title {
  font-size: 1.1rem; font-weight: 700;
  color: var(--text-primary);
  letter-spacing: -0.01em;
}
.hero-status__subtitle {
  font-size: 0.82rem;
  color: var(--text-secondary);
  margin-top: 2px;
}
.hero-status__port {
  color: var(--text-muted);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
}
.hero-status__health {
  font-size: 0.75rem;
  margin-left: 8px;
}
.hero-status__health--ok { color: var(--status-ok); }
.hero-status__health--wait { color: var(--plaw-accent); }
.hero-status__uptime {
  color: var(--text-muted);
  font-size: 0.75rem;
  margin-left: 8px;
  font-family: 'Cascadia Code', 'Fira Code', monospace;
}

/* Stop button */
.stop-btn { color: var(--status-err) !important; }

/* --- Stats Row --- */
.stats-row {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 16px;
}
.stat-item {
  display: flex; align-items: center; gap: 14px;
  padding: 18px 20px;
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-card);
  transition: all var(--duration-normal) var(--ease-out);
}
.stat-item:hover {
  border-color: var(--border-default);
  box-shadow: var(--shadow-card-hover);
  transform: translateY(-1px);
}
.stat-item__icon {
  width: 40px; height: 40px;
  display: flex; align-items: center; justify-content: center;
  border-radius: var(--radius-sm);
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
  flex-shrink: 0;
}
.stat-item__icon--amber {
  background: var(--plaw-accent-soft);
  color: var(--plaw-accent);
}
.stat-item__icon--blue {
  background: var(--status-info-soft);
  color: var(--status-info);
}
.stat-item__icon--green {
  background: var(--status-ok-soft);
  color: var(--status-ok);
}
.stat-item__value {
  font-size: 1.15rem; font-weight: 700;
  color: var(--text-primary);
  letter-spacing: -0.01em;
}
.stat-item__label {
  font-size: 0.78rem;
  color: var(--text-muted);
  margin-top: 1px;
}

/* --- Error bar --- */
.error-bar {
  padding: 10px 16px;
  background: var(--status-err-soft);
  border: 1px solid var(--status-err);
  border-radius: var(--radius-sm);
  color: var(--status-err);
  font-size: 0.85rem;
  margin-bottom: 16px;
}

/* --- Quick start --- */
.quick-start {
  display: flex; align-items: center; justify-content: space-between;
  gap: 16px;
}
.quick-start__title {
  font-size: 1rem; font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 4px;
}
.quick-start__desc {
  font-size: 0.85rem;
  color: var(--text-secondary);
}

/* --- Storage row --- */
.storage-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 20px;
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-card);
}
.storage-row__info {
  display: flex;
  align-items: center;
  gap: 12px;
}
.storage-row__label {
  font-size: 0.85rem;
  font-weight: 500;
  color: var(--text-primary);
}
.storage-row__detail {
  font-size: 0.78rem;
  color: var(--text-muted);
}
</style>
