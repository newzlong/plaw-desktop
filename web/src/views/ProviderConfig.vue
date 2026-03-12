<template>
  <div>
    <h1 class="page-title">{{ t('provider.title') }}</h1>

    <div class="max-w-xl space-y-5">
      <GlassCard :hoverable="false">
        <GlassSelect
          v-model="form.provider"
          :label="t('provider.label')"
          :options="providerOptions"
        />

        <GlassInput
          v-model="form.apiKey"
          :label="t('provider.apiKey')"
          type="password"
          :placeholder="apiKeyEncrypted && !form.apiKey ? t('provider.apiKeyEncrypted') : 'sk-...'"
          :hint="form.provider.startsWith('kimi') ? 'Kimi API Key (sk-...)' : ''"
          class="mt-4"
          @input="apiKeyEncrypted = false"
        />

        <GlassInput
          v-if="form.provider === 'custom' || form.provider === 'ollama'"
          v-model="form.baseUrl"
          :label="t('provider.baseUrl')"
          placeholder="https://api.example.com/v1"
          class="mt-4"
        />

        <GlassSelect
          v-if="modelOptions.length"
          v-model="form.model"
          :label="t('provider.model')"
          :options="modelOptions"
          class="mt-4"
        />

        <GlassInput
          v-else
          v-model="form.model"
          :label="t('provider.modelName')"
          placeholder="model-name"
          class="mt-4"
        />

      </GlassCard>

      <GlassCard :hoverable="false">
        <button class="proxy-toggle" @click="showProxy = !showProxy">
          <span>{{ t('provider.proxyTitle') }}</span>
          <span class="proxy-toggle__arrow" :class="{ 'proxy-toggle__arrow--open': showProxy }">▸</span>
        </button>
        <div v-if="showProxy" class="mt-4">
          <GlassInput
            v-model="form.proxy"
            :label="t('provider.proxyUrl')"
            placeholder="http://127.0.0.1:8118"
            :hint="t('provider.proxyHint')"
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
        <span v-if="testMsg" class="save-msg" :class="testOk ? 'save-msg--ok' : 'save-msg--err'">
          {{ testMsg }}
        </span>
        <span v-if="saveMsg" class="save-msg" :class="saveOk ? 'save-msg--ok' : 'save-msg--err'">
          {{ saveMsg }}
        </span>
        <GlassButton variant="ghost" @click="testConnection" :loading="testing" :disabled="!form.apiKey">
          {{ t('provider.testConnection') }}
        </GlassButton>
        <GlassButton variant="primary" @click="save" :loading="saving">
          {{ t('common.save') }}
        </GlassButton>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted } from 'vue'
import { GlassCard, GlassButton, GlassInput, GlassSelect } from '../components/glass'
import { readConfig, writeConfig, restartPlaw, getPlawStatus, testProviderConnection } from '../api/tauri'
import { useI18n } from '../composables/useI18n'

const { t } = useI18n()

const form = ref({ provider: 'kimi-coder', apiKey: '', baseUrl: '', model: '', proxy: '' })
const apiKeyEncrypted = ref(false) // API Key exists in config but encrypted by Plaw
const showProxy = ref(false)
const saving = ref(false)
const saveMsg = ref('')
const saveOk = ref(false)
const needRestart = ref(false)
const restarting = ref(false)
const testing = ref(false)
const testMsg = ref('')
const testOk = ref(false)

const providerOptions = [
  { label: 'Kimi Coder K2.5 (Default)', value: 'kimi-coder' },
  { label: 'Kimi K2.5 (Moonshot)', value: 'kimi-moonshot' },
  { label: 'Anthropic (Claude)', value: 'anthropic' },
  { label: 'OpenAI', value: 'openai' },
  { label: 'OpenRouter', value: 'openrouter' },
  { label: 'Ollama (Local)', value: 'ollama' },
  { label: 'Custom URL', value: 'custom' },
]

const MODELS = {
  'kimi-coder': [
    { label: 'Kimi K2.5', value: 'k2p5' },
  ],
  'kimi-moonshot': [
    { label: 'Kimi K2.5', value: 'kimi-k2.5' },
  ],
  anthropic: [
    { label: 'Claude 3.5 Sonnet', value: 'claude-3-5-sonnet-20241022' },
    { label: 'Claude 3.5 Haiku', value: 'claude-3-5-haiku-20241022' },
  ],
  openai: [
    { label: 'GPT-4o', value: 'gpt-4o' },
    { label: 'GPT-4o Mini', value: 'gpt-4o-mini' },
  ],
}

const modelOptions = computed(() => MODELS[form.value.provider] || [])

onMounted(async () => {
  try {
    const cfg = await readConfig()
    if (cfg.default_provider) {
      // Detect Kimi provider formats
      if (cfg.default_provider.includes('api.kimi.com/coding')) {
        form.value.provider = 'kimi-coder'
      } else if (cfg.default_provider.includes('moonshot.cn')) {
        form.value.provider = 'kimi-moonshot'
      } else {
        form.value.provider = cfg.default_provider
      }
    }
    if (cfg.api_key) {
      if (String(cfg.api_key).startsWith('enc2:')) {
        apiKeyEncrypted.value = true
      } else {
        form.value.apiKey = cfg.api_key
      }
    }
    if (cfg.provider_api) form.value.baseUrl = cfg.provider_api
    if (cfg.default_model) form.value.model = cfg.default_model
    const rawProxy = cfg.proxy?.https_proxy || cfg.proxy?.http_proxy || ''
    if (rawProxy && !rawProxy.startsWith('enc2:')) {
      form.value.proxy = rawProxy
      showProxy.value = true
    }
  } catch { /* no config yet */ }
})

async function save() {
  saving.value = true
  try {
    const cfg = {}
    try { Object.assign(cfg, await readConfig()) } catch {}
    const KIMI_PROVIDERS = {
      'kimi-coder': { url: 'anthropic-custom:https://api.kimi.com/coding', model: 'k2p5' },
      'kimi-moonshot': { url: 'anthropic-custom:https://api.moonshot.cn', model: 'kimi-k2.5' },
    }
    const kimiCfg = KIMI_PROVIDERS[form.value.provider]
    cfg.default_provider = kimiCfg ? kimiCfg.url : form.value.provider
    // Only overwrite api_key if user entered a new one (not encrypted placeholder)
    if (form.value.apiKey) {
      cfg.api_key = form.value.apiKey
    }
    cfg.default_model = kimiCfg ? kimiCfg.model : form.value.model
    cfg.default_temperature = 0.7
    if (kimiCfg) {
      cfg.provider = { reasoning_level: 'medium' }
    }
    if (!kimiCfg && form.value.baseUrl) cfg.provider_api = form.value.baseUrl
    if (form.value.proxy) {
      cfg.proxy = { https_proxy: form.value.proxy, http_proxy: form.value.proxy }
    } else if (!cfg.proxy || !String(cfg.proxy?.https_proxy || '').startsWith('enc2:')) {
      // Only delete proxy if it's not an encrypted value we couldn't display
      delete cfg.proxy
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

async function testConnection() {
  testing.value = true
  testMsg.value = t('provider.testing')
  testOk.value = false
  try {
    await testProviderConnection(form.value.provider, form.value.apiKey, form.value.baseUrl, form.value.model, form.value.proxy)
    testOk.value = true
    testMsg.value = t('provider.testOk')
  } catch (e) {
    const msg = typeof e === 'string' ? e : (e?.message || '')
    if (msg.includes('auth_failed')) {
      testMsg.value = t('provider.testFailed') + ' (401)'
    } else if (msg.includes('http_error')) {
      const code = msg.split(':')[1] || ''
      testMsg.value = t('provider.testFailed') + ` (${code})`
    } else {
      testMsg.value = t('provider.testFailed') + ': ' + (msg || 'Network error')
    }
  } finally {
    testing.value = false
    setTimeout(() => { testMsg.value = '' }, 5000)
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
  margin-bottom: 24px; letter-spacing: -0.02em;
}
.save-msg {
  font-size: 0.82rem; font-weight: 500;
  transition: opacity 0.3s;
}
.save-msg--ok { color: var(--status-ok); }
.save-msg--err { color: var(--status-err); }
.proxy-toggle {
  display: flex; align-items: center; justify-content: space-between;
  width: 100%; background: none; border: none; cursor: pointer;
  color: var(--text-secondary); font-size: 0.85rem; font-weight: 500;
  padding: 0;
}
.proxy-toggle:hover { color: var(--text-primary); }
.proxy-toggle__arrow {
  transition: transform var(--duration-fast) var(--ease-out);
  font-size: 0.75rem;
}
.proxy-toggle__arrow--open { transform: rotate(90deg); }
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
