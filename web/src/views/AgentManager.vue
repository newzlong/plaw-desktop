<template>
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{{ t('agents.title') }}</h1>
        <p class="page-desc">{{ t('agents.desc') }}</p>
      </div>
      <div class="flex items-center gap-2">
        <GlassButton size="sm" variant="ghost" @click="loadAgents">
          <RefreshCw class="w-4 h-4" />
        </GlassButton>
        <GlassButton size="sm" variant="primary" @click="openCreate">
          <Plus class="w-4 h-4" />
          {{ t('agents.newAgent') }}
        </GlassButton>
      </div>
    </div>

    <!-- Empty state -->
    <GlassCard v-if="!agents.length" :hoverable="false">
      <div class="empty-hint">
        <Users class="w-5 h-5" style="color: var(--text-muted)" />
        <span>{{ t('agents.empty') }}</span>
      </div>
    </GlassCard>

    <!-- Agent list -->
    <div v-else class="space-y-3">
      <GlassCard v-for="agent in agents" :key="agent.name" :hoverable="false">
        <div class="agent-row">
          <div class="agent-info">
            <div class="agent-name">{{ agent.name }}</div>
            <div class="agent-meta">
              <span class="agent-provider">{{ agent.provider || 'default' }}</span>
              <span class="agent-model">{{ agent.model || '--' }}</span>
            </div>
          </div>
          <div class="agent-actions">
            <GlassButton size="sm" variant="ghost" @click="openEdit(agent)">
              <Pencil class="w-3.5 h-3.5" />
            </GlassButton>
            <GlassButton size="sm" variant="ghost" class="delete-btn" @click="confirmDelete(agent.name)">
              <Trash2 class="w-3.5 h-3.5" />
            </GlassButton>
          </div>
        </div>
        <div v-if="agent.system_prompt" class="agent-prompt">
          {{ agent.system_prompt.length > 120 ? agent.system_prompt.slice(0, 120) + '...' : agent.system_prompt }}
        </div>
      </GlassCard>
    </div>

    <!-- Create/Edit dialog -->
    <GlassDialog v-model="showForm" :title="editingName ? t('agents.editAgent') : t('agents.createAgent')">
      <div class="dialog-form">
        <GlassInput
          v-model="form.name"
          :label="t('agents.name')"
          placeholder="my-agent"
          :disabled="!!editingName"
        />
        <GlassSelect
          v-model="form.provider"
          :label="t('agents.provider')"
          :options="providerOptions"
        />
        <GlassInput
          v-model="form.model"
          :label="t('agents.model')"
          :placeholder="form.provider ? 'Model name' : t('agents.inheritDefault')"
          :disabled="!form.provider"
        />
        <div>
          <label class="field-label">{{ t('agents.systemPrompt') }}</label>
          <textarea
            v-model="form.system_prompt"
            class="glass-textarea"
            rows="4"
            placeholder="You are a helpful assistant..."
          />
        </div>
        <GlassInput
          v-model="form.max_iterations"
          :label="t('agents.maxIterations')"
          type="number"
          placeholder="25"
        />
        <GlassTag
          v-model="form.tools"
          :label="t('agents.allowedTools')"
          placeholder="e.g. web_search, shell..."
        />
      </div>
      <template #footer>
        <GlassButton variant="ghost" @click="showForm = false">{{ t('common.cancel') }}</GlassButton>
        <GlassButton variant="primary" :loading="saving" @click="saveAgent">
          {{ editingName ? t('common.update') : t('common.create') }}
        </GlassButton>
      </template>
    </GlassDialog>

    <!-- Delete confirm -->
    <GlassDialog v-model="showDeleteConfirm" :title="t('agents.deleteTitle')">
      <p class="dialog-hint">
        {{ t('agents.deleteConfirm') }} <strong>{{ deleteName }}</strong>?
      </p>
      <template #footer>
        <GlassButton variant="ghost" @click="showDeleteConfirm = false">{{ t('common.cancel') }}</GlassButton>
        <GlassButton variant="danger" @click="deleteAgent">
          {{ t('common.delete') }}
        </GlassButton>
      </template>
    </GlassDialog>
  </div>
</template>

<script setup>
import { ref, watch, onMounted } from 'vue'
import { Plus, Users, Pencil, Trash2, RefreshCw } from 'lucide-vue-next'
import { GlassCard, GlassButton, GlassInput, GlassSelect, GlassDialog, GlassTag } from '../components/glass'
import { useI18n } from '../composables/useI18n'
import { readConfig, writeConfig } from '../api/tauri'

const { t } = useI18n()

const agents = ref([])
const showForm = ref(false)
const showDeleteConfirm = ref(false)
const deleteName = ref('')
const editingName = ref(null)
const saving = ref(false)

const form = ref({
  name: '', provider: '', model: '',
  system_prompt: '', max_iterations: '25', tools: [],
})

const providerOptions = [
  { label: 'Default (inherit)', value: '' },
  { label: 'Kimi Coder', value: 'anthropic-custom:https://api.kimi.com/coding' },
  { label: 'Kimi Moonshot', value: 'anthropic-custom:https://api.moonshot.cn' },
  { label: 'Anthropic', value: 'anthropic' },
  { label: 'OpenAI', value: 'openai' },
  { label: 'DeepSeek', value: 'deepseek' },
  { label: 'OpenRouter', value: 'openrouter' },
]

// Clear model when switching to Default (inherit)
watch(() => form.value.provider, (val) => {
  if (!val) form.value.model = ''
})

async function loadAgents() {
  try {
    const cfg = await readConfig()
    if (cfg.agents && typeof cfg.agents === 'object') {
      agents.value = Object.entries(cfg.agents).map(([name, conf]) => ({
        name,
        ...conf,
      }))
    }
  } catch { /* no config */ }
}

function openCreate() {
  editingName.value = null
  form.value = { name: '', provider: '', model: '', system_prompt: '', max_iterations: '25', tools: [] }
  showForm.value = true
}

// Migrate legacy Tauri-only aliases to Plaw provider format
const PROVIDER_MIGRATION = {
  'kimi-coder': 'anthropic-custom:https://api.kimi.com/coding',
  'kimi-moonshot': 'anthropic-custom:https://api.moonshot.cn',
}

function openEdit(agent) {
  editingName.value = agent.name
  const provider = PROVIDER_MIGRATION[agent.provider] || agent.provider || ''
  form.value = {
    name: agent.name,
    provider,
    model: agent.model || '',
    system_prompt: agent.system_prompt || '',
    max_iterations: String(agent.max_iterations || 25),
    tools: agent.tools || [],
  }
  showForm.value = true
}

async function saveAgent() {
  saving.value = true
  try {
    const cfg = {}
    try { Object.assign(cfg, await readConfig()) } catch {}
    if (!cfg.agents) cfg.agents = {}

    const name = editingName.value || form.value.name.trim()
    if (!name) return

    const agentCfg = {}
    if (form.value.provider) agentCfg.provider = form.value.provider
    if (form.value.model) agentCfg.model = form.value.model
    if (form.value.system_prompt) agentCfg.system_prompt = form.value.system_prompt
    const maxIter = parseInt(form.value.max_iterations)
    if (maxIter && maxIter !== 25) agentCfg.max_iterations = maxIter
    if (form.value.tools.length) agentCfg.tools = form.value.tools

    cfg.agents[name] = agentCfg
    await writeConfig(cfg)
    await loadAgents()
    showForm.value = false
  } finally { saving.value = false }
}

function confirmDelete(name) {
  deleteName.value = name
  showDeleteConfirm.value = true
}

async function deleteAgent() {
  try {
    const cfg = {}
    try { Object.assign(cfg, await readConfig()) } catch {}
    if (cfg.agents) {
      delete cfg.agents[deleteName.value]
      await writeConfig(cfg)
    }
    await loadAgents()
  } finally { showDeleteConfirm.value = false }
}

onMounted(loadAgents)
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

.agent-row {
  display: flex; align-items: center; justify-content: space-between;
}
.agent-name {
  font-size: 0.95rem; font-weight: 600;
  color: var(--text-primary);
}
.agent-meta {
  display: flex; gap: 8px; margin-top: 2px;
}
.agent-provider, .agent-model {
  font-size: 0.75rem;
  color: var(--text-muted);
  background: var(--input-bg);
  padding: 1px 8px; border-radius: 8px;
}
.agent-actions { display: flex; gap: 4px; }
.delete-btn { color: var(--status-err) !important; }

.agent-prompt {
  margin-top: 8px; padding-top: 8px;
  border-top: 1px solid var(--border-subtle);
  font-size: 0.8rem; color: var(--text-muted);
  font-style: italic;
}
</style>
