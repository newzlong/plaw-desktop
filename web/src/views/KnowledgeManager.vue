<template>
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{{ t('knowledge.title') }}</h1>
        <p class="page-desc">{{ t('knowledge.desc') }}</p>
      </div>
      <div class="header-actions">
        <GlassButton size="sm" variant="ghost" @click="loadStats(); doSearch()">
          <RefreshCw class="w-4 h-4" />
        </GlassButton>
        <GlassButton size="sm" variant="primary" @click="openCreateDialog">
          <Plus class="w-4 h-4" />
          {{ t('knowledge.create') }}
        </GlassButton>
        <GlassButton size="sm" variant="ghost" @click="openFolder">
          <FolderOpen class="w-4 h-4" />
          {{ t('knowledge.openFolder') }}
        </GlassButton>
      </div>
    </div>

    <!-- Stats -->
    <div class="stats-row" v-if="stats.total_entries > 0">
      <div class="stat-item">
        <span class="stat-value">{{ stats.total_entries }}</span>
        <span class="stat-label">{{ t('knowledge.entries') }}</span>
      </div>
      <div class="stat-item">
        <span class="stat-value">{{ stats.total_tags }}</span>
        <span class="stat-label">{{ t('knowledge.tags') }}</span>
      </div>
    </div>

    <!-- Top Tags -->
    <div class="tags-row" v-if="stats.top_tags?.length">
      <span
        v-for="[tag, count] in stats.top_tags" :key="tag"
        class="tag-chip"
        :class="{ 'tag-chip--active': filterTag === tag }"
        @click="toggleTag(tag)"
      >{{ tag }} ({{ count }})</span>
    </div>

    <!-- Search -->
    <GlassInput
      v-model="query"
      :placeholder="t('knowledge.searchPlaceholder')"
      class="mb-4"
      @input="debouncedSearch"
    />

    <!-- Empty state -->
    <GlassCard v-if="!entries.length && !loading" :hoverable="false">
      <div class="empty-hint">
        <BookOpen class="w-5 h-5" style="color: var(--text-muted)" />
        <span>{{ t('knowledge.empty') }}</span>
      </div>
    </GlassCard>

    <!-- Entry list -->
    <div v-else class="space-y-3">
      <GlassCard
        v-for="entry in entries" :key="entry.id"
        :hoverable="true"
        @click="selectedEntry = entry; showDetail = true"
      >
        <div class="entry-row">
          <div class="entry-info">
            <div class="entry-title">{{ entry.title }}</div>
            <div class="entry-preview">{{ entry.preview }}</div>
            <div class="entry-meta">
              <span v-if="entry.updated" class="entry-date">{{ entry.updated }}</span>
              <span v-if="entry.source" class="entry-source">{{ entry.source }}</span>
              <span v-for="tag in entry.tags" :key="tag" class="entry-tag">{{ tag }}</span>
            </div>
          </div>
          <GlassButton
            size="sm" variant="ghost" class="delete-btn"
            @click.stop="confirmDelete(entry)"
          >
            <Trash2 class="w-3.5 h-3.5" />
          </GlassButton>
        </div>
      </GlassCard>
    </div>

    <!-- Detail dialog -->
    <GlassDialog v-model="showDetail" :title="selectedEntry?.title || ''">
      <div v-if="detailContent" class="detail-content">
        <div class="detail-meta">
          <span v-if="selectedEntry?.source">
            {{ t('knowledge.source') }}: {{ selectedEntry.source }}
          </span>
          <span v-if="selectedEntry?.created">
            {{ t('knowledge.created') }}: {{ selectedEntry.created }}
          </span>
          <span v-if="selectedEntry?.updated">
            {{ t('knowledge.updated') }}: {{ selectedEntry.updated }}
          </span>
        </div>
        <div class="detail-tags" v-if="selectedEntry?.tags?.length">
          <span v-for="tag in selectedEntry.tags" :key="tag" class="entry-tag">{{ tag }}</span>
        </div>
        <pre class="detail-body">{{ detailContent }}</pre>
      </div>
      <template #footer>
        <GlassButton variant="ghost" @click="showDetail = false">
          {{ t('common.cancel') }}
        </GlassButton>
        <GlassButton variant="primary" @click="openEditDialog">
          {{ t('knowledge.editTitle') }}
        </GlassButton>
      </template>
    </GlassDialog>

    <!-- Create/Edit dialog -->
    <GlassDialog v-model="showEditor" :title="editId ? t('knowledge.editTitle') : t('knowledge.createTitle')">
      <div class="dialog-form">
        <GlassInput
          v-model="editTitle"
          :label="t('knowledge.entryTitle')"
          :placeholder="t('knowledge.entryTitle')"
        />
        <GlassInput
          v-model="editTagsStr"
          :label="t('knowledge.entryTags')"
          :placeholder="t('knowledge.entryTags')"
        />
        <div>
          <label class="field-label">{{ t('knowledge.entryContent') }}</label>
          <textarea
            v-model="editContent"
            :placeholder="t('knowledge.entryContent')"
            class="glass-textarea"
            rows="10"
          />
        </div>
      </div>
      <template #footer>
        <GlassButton variant="ghost" @click="showEditor = false">
          {{ t('common.cancel') }}
        </GlassButton>
        <GlassButton variant="primary" @click="doSave" :disabled="!editTitle.trim()">
          {{ t('common.save') }}
        </GlassButton>
      </template>
    </GlassDialog>

    <!-- Delete confirmation -->
    <GlassDialog v-model="showDeleteConfirm" :title="t('knowledge.deleteTitle')">
      <p class="dialog-hint">{{ t('knowledge.deleteConfirm') }}</p>
      <template #footer>
        <GlassButton variant="ghost" @click="showDeleteConfirm = false">
          {{ t('common.cancel') }}
        </GlassButton>
        <GlassButton variant="danger" @click="doDelete">
          {{ t('common.delete') }}
        </GlassButton>
      </template>
    </GlassDialog>
  </div>
</template>

<script setup>
import { ref, onMounted, watch } from 'vue'
import { BookOpen, FolderOpen, Plus, Trash2, RefreshCw } from 'lucide-vue-next'
import { GlassCard, GlassButton, GlassInput, GlassDialog } from '../components/glass'
import { useI18n } from '../composables/useI18n'
import {
  searchKnowledge, readKnowledgeEntry, deleteKnowledgeEntry, getKnowledgeStats,
  saveKnowledgeEntry,
} from '../api/tauri'

const { t } = useI18n()

const query = ref('')
const filterTag = ref('')
const entries = ref([])
const loading = ref(false)
const stats = ref({ total_entries: 0, total_tags: 0, top_tags: [], dir_path: '' })

const showDetail = ref(false)
const selectedEntry = ref(null)
const detailContent = ref('')

const showDeleteConfirm = ref(false)
const deleteTarget = ref(null)

const showEditor = ref(false)
const editId = ref(null)
const editTitle = ref('')
const editTagsStr = ref('')
const editContent = ref('')

let debounceTimer = null
function debouncedSearch() {
  clearTimeout(debounceTimer)
  debounceTimer = setTimeout(() => doSearch(), 300)
}

async function doSearch() {
  loading.value = true
  try {
    const q = filterTag.value ? filterTag.value : query.value
    entries.value = await searchKnowledge(q)
  } catch {
    entries.value = []
  } finally {
    loading.value = false
  }
}

function toggleTag(tag) {
  filterTag.value = filterTag.value === tag ? '' : tag
  doSearch()
}

async function loadStats() {
  stats.value = await getKnowledgeStats()
}

watch(showDetail, async (val) => {
  if (val && selectedEntry.value) {
    try {
      const [, body] = await readKnowledgeEntry(selectedEntry.value.id)
      detailContent.value = body
    } catch {
      detailContent.value = 'Failed to load content'
    }
  }
})

function confirmDelete(entry) {
  deleteTarget.value = entry
  showDeleteConfirm.value = true
}

async function doDelete() {
  if (!deleteTarget.value) return
  try {
    await deleteKnowledgeEntry(deleteTarget.value.id)
    showDeleteConfirm.value = false
    deleteTarget.value = null
    await doSearch()
    await loadStats()
  } catch (e) {
    alert(typeof e === 'string' ? e : (e?.message || 'Delete failed'))
  }
}

function openCreateDialog() {
  editId.value = null
  editTitle.value = ''
  editTagsStr.value = ''
  editContent.value = ''
  showEditor.value = true
}

async function openEditDialog() {
  if (!selectedEntry.value) return
  editId.value = selectedEntry.value.id
  editTitle.value = selectedEntry.value.title
  editTagsStr.value = (selectedEntry.value.tags || []).join(', ')
  editContent.value = detailContent.value || ''
  showDetail.value = false
  showEditor.value = true
}

async function doSave() {
  const tags = editTagsStr.value
    .split(',')
    .map(t => t.trim())
    .filter(t => t.length > 0)
  try {
    await saveKnowledgeEntry(editTitle.value.trim(), tags, editContent.value, editId.value)
    showEditor.value = false
    await doSearch()
    await loadStats()
  } catch (e) {
    alert(typeof e === 'string' ? e : (e?.message || t('knowledge.saveFailed')))
  }
}

async function openFolder() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    if (stats.value.dir_path) {
      await invoke('plugin:opener|open_path', { path: stats.value.dir_path })
    }
  } catch { /* ignore */ }
}

onMounted(async () => {
  await loadStats()
  await doSearch()
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

.stats-row {
  display: flex; gap: 16px; margin-bottom: 16px;
}
.stat-item {
  display: flex; align-items: baseline; gap: 6px;
}
.stat-value {
  font-size: 1.5rem; font-weight: 700;
  color: var(--lobster-primary);
}
.stat-label {
  font-size: 0.8rem; color: var(--text-muted);
}

.tags-row {
  display: flex; flex-wrap: wrap; gap: 6px; margin-bottom: 16px;
}
.tag-chip {
  padding: 3px 10px; border-radius: 999px;
  font-size: 0.75rem; font-weight: 500;
  background: var(--bg-raised);
  color: var(--text-secondary);
  border: 1px solid var(--border-subtle);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.tag-chip:hover {
  border-color: var(--lobster-primary);
  color: var(--lobster-primary);
}
.tag-chip--active {
  background: var(--lobster-primary-soft);
  border-color: var(--lobster-primary);
  color: var(--lobster-primary);
}

.empty-hint {
  display: flex; align-items: center; gap: 10px;
  color: var(--text-secondary); font-size: 0.875rem;
}

.entry-row {
  display: flex; align-items: flex-start; justify-content: space-between;
}
.entry-info { flex: 1; min-width: 0; }
.entry-title {
  font-size: 0.95rem; font-weight: 600;
  color: var(--text-primary);
}
.entry-preview {
  font-size: 0.8rem; color: var(--text-muted); margin-top: 4px;
  overflow: hidden; text-overflow: ellipsis;
  display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical;
}
.entry-meta {
  display: flex; flex-wrap: wrap; gap: 8px; margin-top: 6px;
  font-size: 0.7rem; color: var(--text-muted);
}
.entry-date { opacity: 0.8; }
.entry-source {
  text-transform: uppercase; letter-spacing: 0.05em;
}
.entry-tag {
  padding: 1px 6px; border-radius: 999px;
  background: var(--lobster-primary-soft);
  color: var(--lobster-primary);
  font-size: 0.7rem;
}

.delete-btn { color: var(--status-err) !important; }

.detail-content { font-size: 0.85rem; }
.detail-meta {
  display: flex; flex-wrap: wrap; gap: 12px;
  color: var(--text-muted); font-size: 0.78rem;
  margin-bottom: 12px;
}
.detail-tags {
  display: flex; flex-wrap: wrap; gap: 6px;
  margin-bottom: 12px;
}
.detail-body {
  white-space: pre-wrap; word-break: break-word;
  font-family: inherit; font-size: 0.85rem;
  color: var(--text-primary);
  background: var(--bg-raised);
  border-radius: var(--radius-sm);
  padding: 12px;
  max-height: 400px; overflow-y: auto;
}

.header-actions {
  display: flex; gap: 8px; align-items: center;
}

.glass-textarea { min-height: 200px; }
</style>
