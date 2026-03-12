<template>
  <div>
    <h1 class="page-title">{{ t('security.title') }}</h1>
    <p class="page-desc">{{ t('security.desc') }}</p>

    <div class="max-w-xl space-y-5">
      <!-- Autonomy Level Presets -->
      <GlassCard :hoverable="false">
        <label class="field-label">{{ t('security.autonomyLevel') }}</label>
        <div class="preset-grid">
          <button
            v-for="p in presets"
            :key="p.value"
            class="preset-card"
            :class="{ 'preset-card--active': form.level === p.value }"
            @click="applyPreset(p)"
          >
            <component :is="p.icon" class="w-5 h-5 mb-2" />
            <div class="preset-card__title">{{ t(p.i18nLabel) }}</div>
            <div class="preset-card__desc">{{ t(p.i18nDesc) }}</div>
          </button>
        </div>
      </GlassCard>

      <!-- Detailed Settings -->
      <GlassCard :hoverable="false">
        <label class="field-label">{{ t('security.detailedSettings') }}</label>

        <div class="setting-row">
          <GlassToggle v-model="form.workspaceOnly" :label="t('security.workspaceOnly')" />
          <span class="setting-hint">{{ t('security.workspaceOnlyHint') }}</span>
        </div>

        <div class="mt-4">
          <GlassTag
            v-model="form.allowedCommands"
            :label="t('security.allowedCommands')"
            placeholder="e.g. git, ls, cat..."
          />
        </div>

        <div class="mt-4">
          <GlassTag
            v-model="form.forbiddenPaths"
            :label="t('security.forbiddenPaths')"
            placeholder="e.g. /etc, C:\\Windows..."
          />
        </div>
      </GlassCard>

    </div>

    <!-- Sticky save bar -->
    <div class="sticky-actions">
      <div v-if="needRestart" class="restart-bar mb-3">
        <span>{{ t('common.restartHint') }}</span>
        <GlassButton size="sm" variant="primary" :loading="restarting" @click="doRestart">
          {{ t('common.restart') }}
        </GlassButton>
      </div>
      <div class="flex items-center justify-end gap-3">
        <span v-if="saveMsg" class="save-msg" :class="saveOk ? 'save-msg--ok' : 'save-msg--err'">
          {{ saveMsg }}
        </span>
        <GlassButton variant="primary" :loading="saving" @click="save">
          {{ t('common.save') }}
        </GlassButton>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, reactive, onMounted } from 'vue'
import { Eye, ShieldCheck, Zap } from 'lucide-vue-next'
import { GlassCard, GlassButton, GlassToggle, GlassTag } from '../components/glass'
import { readConfig, writeConfig, restartPlaw, getPlawStatus } from '../api/tauri'
import { useI18n } from '../composables/useI18n'
const { t } = useI18n()

const saving = ref(false)
const saveMsg = ref('')
const saveOk = ref(false)
const needRestart = ref(false)
const restarting = ref(false)

const form = reactive({
  level: 'supervised',
  workspaceOnly: true,
  allowedCommands: [],
  forbiddenPaths: [],
})

const presets = [
  {
    value: 'readonly',
    i18nLabel: 'security.conservative',
    i18nDesc: 'security.conservativeDesc',
    icon: Eye,
    config: { workspaceOnly: true, allowedCommands: [], forbiddenPaths: [] },
  },
  {
    value: 'supervised',
    i18nLabel: 'security.standard',
    i18nDesc: 'security.standardDesc',
    icon: ShieldCheck,
    config: {
      workspaceOnly: true,
      allowedCommands: ['git', 'ls', 'cat', 'grep', 'find', 'head', 'tail', 'wc'],
      forbiddenPaths: [],
    },
  },
  {
    value: 'full',
    i18nLabel: 'security.permissive',
    i18nDesc: 'security.permissiveDesc',
    icon: Zap,
    config: { workspaceOnly: false, allowedCommands: ['*'], forbiddenPaths: [] },
  },
]

function applyPreset(p) {
  form.level = p.value
  form.workspaceOnly = p.config.workspaceOnly
  form.allowedCommands = [...p.config.allowedCommands]
  form.forbiddenPaths = [...p.config.forbiddenPaths]
}

onMounted(async () => {
  try {
    const cfg = await readConfig()
    if (cfg.autonomy) {
      form.level = cfg.autonomy.level || 'supervised'
      form.workspaceOnly = cfg.autonomy.workspace_only !== false
      form.allowedCommands = cfg.autonomy.allowed_commands || []
      form.forbiddenPaths = cfg.autonomy.forbidden_paths || []
      // Full autonomy uses wildcard — normalize loaded commands
      if (form.level === 'full' && !form.allowedCommands.includes('*')) {
        form.allowedCommands = ['*']
      }
    }
  } catch { /* no config yet */ }
})

async function save() {
  saving.value = true
  try {
    const cfg = {}
    try { Object.assign(cfg, await readConfig()) } catch {}

    cfg.autonomy = {
      ...cfg.autonomy,
      level: form.level,
      workspace_only: form.workspaceOnly,
      allowed_commands: form.allowedCommands || [],
      forbidden_paths: form.forbiddenPaths || [],
      max_actions_per_hour: cfg.autonomy?.max_actions_per_hour || 1000,
      max_cost_per_day_cents: cfg.autonomy?.max_cost_per_day_cents || 10000,
    }

    // Sync network/tool permissions based on autonomy level
    if (form.level === 'full') {
      cfg.autonomy.block_high_risk_commands = false
      cfg.autonomy.non_cli_excluded_tools = []  // Allow all tools including shell via WebSocket
      cfg.web_fetch = { ...cfg.web_fetch, allowed_domains: ['*'], enabled: true }
      cfg.http_request = { ...cfg.http_request, allowed_domains: ['*'], allow_local: true, enabled: true }
      cfg.browser = { ...cfg.browser, enabled: true, allowed_domains: ['*'] }
    } else if (form.level === 'supervised') {
      cfg.autonomy.block_high_risk_commands = true
      cfg.autonomy.non_cli_excluded_tools = [
        'shell', 'file_write', 'file_edit', 'git_operations',
        'browser', 'browser_open', 'memory_forget',
      ]
      cfg.web_fetch = { ...cfg.web_fetch, allowed_domains: ['*'], enabled: true }
      cfg.http_request = { ...cfg.http_request, allowed_domains: ['localhost', '127.0.0.1'], allow_local: true, enabled: true }
      cfg.browser = { ...cfg.browser, enabled: false }
    } else {
      // readonly
      cfg.autonomy.block_high_risk_commands = true
      cfg.autonomy.non_cli_excluded_tools = [
        'shell', 'file_write', 'file_edit', 'git_operations',
        'browser', 'browser_open', 'http_request',
        'schedule', 'cron_add', 'cron_remove', 'cron_update', 'cron_run',
        'memory_store', 'memory_forget', 'proxy_config', 'model_routing_config',
      ]
      cfg.web_fetch = { ...cfg.web_fetch, allowed_domains: [], enabled: false }
      cfg.http_request = { ...cfg.http_request, allowed_domains: [], allow_local: false, enabled: false }
      cfg.browser = { ...cfg.browser, enabled: false, allowed_domains: [] }
    }

    await writeConfig(cfg)
    saveOk.value = true
    saveMsg.value = t('common.saved')
    try {
      const status = await getPlawStatus()
      if (status) needRestart.value = true
    } catch {}
  } catch (e) {
    saveOk.value = false
    saveMsg.value = e?.message || t('common.saveFailed')
  } finally {
    saving.value = false
    setTimeout(() => { saveMsg.value = '' }, 3000)
  }
}

async function doRestart() {
  restarting.value = true
  try {
    await restartPlaw()
    needRestart.value = false
  } catch { /* ignore */ }
  finally { restarting.value = false }
}
</script>

<style scoped>
.page-title {
  font-size: 1.5rem; font-weight: 700;
  color: var(--text-primary);
  margin-bottom: 4px; letter-spacing: -0.02em;
}
.page-desc {
  color: var(--text-secondary);
  font-size: 0.875rem;
  margin-bottom: 24px;
}
.field-label {
  display: block;
  font-size: 0.8rem; font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 12px;
}

.preset-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 12px;
}
.preset-card {
  display: flex; flex-direction: column; align-items: center;
  background: var(--bg-raised);
  border: 2px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: 1rem; text-align: center;
  cursor: pointer; color: var(--text-primary);
  transition: all var(--duration-fast) var(--ease-out);
}
.preset-card:hover { border-color: var(--border-strong); }
.preset-card--active {
  border-color: var(--lobster-primary);
  background: var(--lobster-primary-soft);
  box-shadow: var(--shadow-glow);
}
.preset-card__title {
  font-size: 0.9rem; font-weight: 600;
  margin-bottom: 4px;
}
.preset-card__desc {
  font-size: 0.75rem;
  color: var(--text-muted);
}

.setting-row {
  display: flex; align-items: center; gap: 12px;
  margin-top: 8px;
}
.setting-hint {
  font-size: 0.78rem;
  color: var(--text-muted);
}

.save-msg { font-size: 0.82rem; font-weight: 500; transition: opacity 0.3s; }
.save-msg--ok { color: var(--status-ok); }
.save-msg--err { color: var(--status-err); }
.restart-bar {
  display: flex; align-items: center; justify-content: space-between;
  background: var(--lobster-primary-soft);
  border: 1px solid var(--lobster-primary);
  border-radius: var(--radius-md);
  padding: 10px 16px;
  font-size: 0.85rem;
  color: var(--text-primary);
}
</style>
