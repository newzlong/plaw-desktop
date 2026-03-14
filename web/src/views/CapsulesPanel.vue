<template>
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{{ t('capsules.title') }}</h1>
        <p class="page-desc">{{ t('capsules.desc') }}</p>
      </div>
      <div class="header-actions">
        <GlassButton size="sm" variant="ghost" @click="load">
          <RefreshCw class="w-4 h-4" />
        </GlassButton>
      </div>
    </div>

    <!-- Stats -->
    <div class="stats-row" v-if="stats.total_count > 0">
      <div class="stat-item">
        <span class="stat-value">{{ stats.total_count }}</span>
        <span class="stat-label">{{ t('capsules.total') }}</span>
      </div>
      <div class="stat-item">
        <span class="stat-value">{{ formatTokens(stats.total_tokens) }}</span>
        <span class="stat-label">{{ t('capsules.totalTokens') }}</span>
      </div>
    </div>

    <!-- Empty state -->
    <GlassCard v-if="!capsules.length && !loading" :hoverable="false">
      <div class="empty-hint">
        <Pill class="w-5 h-5" style="color: var(--text-muted)" />
        <span>{{ t('capsules.empty') }}</span>
      </div>
    </GlassCard>

    <!-- Capsule grid -->
    <div v-else class="capsule-grid">
      <div
        v-for="cap in capsules"
        :key="cap.id"
        class="capsule-item"
        :class="{ 'capsule-item--expanded': expandedId === cap.id }"
        @click="toggleExpand(cap.id)"
      >
        <!-- Capsule pill shape -->
        <div class="capsule-pill">
          <div class="capsule-pill__glow"></div>
          <div class="capsule-pill__body">
            <div class="capsule-pill__top">
              <span class="capsule-pill__date">{{ formatDate(cap.created_at) }}</span>
              <span class="capsule-pill__count">{{ cap.message_count }} {{ t('capsules.messages') }}</span>
            </div>
            <div class="capsule-pill__keywords">
              <span
                v-for="kw in cap.keywords.slice(0, 5)"
                :key="kw"
                class="capsule-keyword"
              >{{ kw }}</span>
            </div>
            <div class="capsule-pill__summary" v-if="expandedId === cap.id">
              {{ cap.summary }}
            </div>
            <div class="capsule-pill__meta">
              <span class="capsule-pill__tokens">{{ formatTokens(cap.token_count) }} {{ t('capsules.tokens') }}</span>
              <button
                class="capsule-pill__delete"
                @click.stop="confirmDelete(cap)"
              >
                <Trash2 class="w-3.5 h-3.5" />
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Delete confirmation -->
    <GlassDialog
      v-model="showDeleteDialog"
      :title="t('capsules.deleteTitle')"
      :message="t('capsules.deleteConfirm')"
      :confirm-text="t('common.confirm')"
      :cancel-text="t('common.cancel')"
      variant="danger"
      @confirm="doDelete"
    />
  </div>
</template>

<script setup>
import { ref, onMounted } from 'vue'
import { useI18n } from '../composables/useI18n'
import { GlassButton, GlassCard, GlassDialog } from '../components/glass'
import { RefreshCw, Pill, Trash2 } from 'lucide-vue-next'

const { invoke } = window.__TAURI__.core

const { t } = useI18n()

const capsules = ref([])
const stats = ref({ total_count: 0, total_tokens: 0 })
const loading = ref(false)
const expandedId = ref(null)
const showDeleteDialog = ref(false)
const deletingCapsule = ref(null)

function formatTokens(n) {
  if (!n) return '0'
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k'
  return String(n)
}

function formatDate(iso) {
  if (!iso) return ''
  try {
    const d = new Date(iso)
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
  } catch {
    return iso.slice(0, 16)
  }
}

function toggleExpand(id) {
  expandedId.value = expandedId.value === id ? null : id
}

async function load() {
  loading.value = true
  try {
    const [capList, capStats] = await Promise.all([
      invoke('list_capsules', { limit: 200 }),
      invoke('get_capsule_stats').catch(() => ({ total_count: 0, total_tokens: 0 })),
    ])
    capsules.value = capList
    stats.value = capStats
  } catch (e) {
    console.warn('[capsules] load failed:', e)
    capsules.value = []
  } finally {
    loading.value = false
  }
}

function confirmDelete(cap) {
  deletingCapsule.value = cap
  showDeleteDialog.value = true
}

async function doDelete() {
  if (!deletingCapsule.value) return
  try {
    await invoke('delete_capsule', { id: deletingCapsule.value.id })
    capsules.value = capsules.value.filter(c => c.id !== deletingCapsule.value.id)
    if (stats.value.total_count > 0) stats.value.total_count--
    stats.value.total_tokens = Math.max(0, stats.value.total_tokens - (deletingCapsule.value.token_count || 0))
  } catch (e) {
    console.error('[capsules] delete failed:', e)
  } finally {
    deletingCapsule.value = null
  }
}

onMounted(load)
</script>

<style scoped>
.stats-row {
  display: flex;
  gap: 16px;
  margin-bottom: 16px;
}
.stat-item {
  display: flex;
  align-items: baseline;
  gap: 6px;
}
.stat-value {
  font-size: 1.2rem;
  font-weight: 700;
  color: var(--plaw-primary);
}
.stat-label {
  font-size: 0.78rem;
  color: var(--text-muted);
}

.empty-hint {
  display: flex;
  align-items: center;
  gap: 10px;
  justify-content: center;
  padding: 24px;
  color: var(--text-muted);
  font-size: 0.88rem;
}

/* ---- Capsule Grid ---- */
.capsule-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 12px;
}

/* ---- Capsule Item ---- */
.capsule-item {
  cursor: pointer;
  transition: transform var(--duration-fast) var(--ease-out);
}
.capsule-item:hover {
  transform: translateY(-2px);
}

/* ---- Pill shape ---- */
.capsule-pill {
  position: relative;
  border-radius: var(--radius-lg);
  overflow: hidden;
  background: var(--card-bg);
  border: 1px solid var(--border-subtle);
  transition: border-color var(--duration-fast) var(--ease-out),
              box-shadow var(--duration-fast) var(--ease-out);
}
.capsule-pill:hover {
  border-color: var(--plaw-primary-soft);
  box-shadow: var(--shadow-card-hover);
}
.capsule-item--expanded .capsule-pill {
  border-color: var(--plaw-primary);
  box-shadow: 0 0 16px rgba(255, 77, 42, 0.12);
}

.capsule-pill__glow {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  height: 3px;
  background: linear-gradient(90deg, var(--plaw-primary), var(--plaw-accent));
  opacity: 0.6;
}
.capsule-item--expanded .capsule-pill__glow {
  opacity: 1;
}

.capsule-pill__body {
  padding: 14px 16px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.capsule-pill__top {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.capsule-pill__date {
  font-size: 0.75rem;
  color: var(--text-muted);
}
.capsule-pill__count {
  font-size: 0.72rem;
  color: var(--text-secondary);
  background: var(--bg-surface);
  padding: 2px 8px;
  border-radius: 10px;
}

.capsule-pill__keywords {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.capsule-keyword {
  font-size: 0.73rem;
  font-weight: 500;
  padding: 2px 8px;
  border-radius: 6px;
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
}

.capsule-pill__summary {
  font-size: 0.8rem;
  color: var(--text-secondary);
  line-height: 1.5;
  max-height: 120px;
  overflow-y: auto;
  white-space: pre-wrap;
  padding: 8px 0 4px;
  border-top: 1px solid var(--border-subtle);
}

.capsule-pill__meta {
  display: flex;
  justify-content: space-between;
  align-items: center;
}
.capsule-pill__tokens {
  font-size: 0.72rem;
  color: var(--text-muted);
}
.capsule-pill__delete {
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  cursor: pointer;
  opacity: 0;
  transition: all var(--duration-fast);
}
.capsule-pill:hover .capsule-pill__delete {
  opacity: 0.6;
}
.capsule-pill__delete:hover {
  opacity: 1 !important;
  color: var(--status-err);
  background: rgba(239, 68, 68, 0.1);
}

.header-actions {
  display: flex;
  gap: 8px;
}
</style>
