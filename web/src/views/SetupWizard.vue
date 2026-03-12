<template>
  <div class="setup-root">
    <div class="setup-container">
      <div class="setup-brand">
        <span class="setup-brand__logo">L</span>
        <h1 class="setup-brand__title">{{ t('setup.welcome') }}</h1>
      </div>
      <p class="setup-subtitle">{{ t('setup.subtitle') }}</p>

      <GlassSteps :steps="[t('setup.stepProvider'), t('setup.stepChannel'), t('setup.stepSecurity')]" :current="step" class="mb-8" />

      <!-- Step 1: Provider -->
      <GlassCard v-if="step === 0" :hoverable="false">
        <GlassSelect
          v-model="form.provider"
          :label="t('provider.label')"
          :options="providerOptions"
        />
        <GlassInput
          v-model="form.apiKey"
          :label="t('provider.apiKey')"
          type="password"
          placeholder="sk-..."
          :hint="form.provider.startsWith('kimi') ? 'Kimi API Key (sk-...)' : ''"
          class="mt-4"
        />
        <GlassInput
          v-if="form.provider === 'custom' || form.provider === 'ollama'"
          v-model="form.baseUrl"
          :label="t('provider.baseUrl')"
          placeholder="https://api.example.com/v1"
          class="mt-4"
        />
        <GlassSelect
          v-if="modelOpts.length"
          v-model="form.model"
          :label="t('provider.model')"
          :options="modelOpts"
          class="mt-4"
        />
        <GlassInput
          v-else
          v-model="form.model"
          :label="t('provider.modelName')"
          placeholder="model-name"
          class="mt-4"
        />
        <div class="flex items-center gap-3 mt-4">
          <GlassButton size="sm" variant="ghost" :loading="testing" :disabled="!form.apiKey" @click="testConnection">
            {{ t('provider.testConnection') }}
          </GlassButton>
          <span v-if="testMsg" class="test-msg" :class="testOk ? 'test-msg--ok' : 'test-msg--err'">{{ testMsg }}</span>
        </div>
      </GlassCard>

      <!-- Step 2: Channel (optional) -->
      <GlassCard v-if="step === 1" :hoverable="false">
        <p class="step-hint">
          {{ t('setup.channelHint') }}
        </p>
        <GlassSelect
          v-model="form.channel"
          :label="t('setup.channelType')"
          :options="channelOptions"
        />
        <GlassInput
          v-if="form.channel === 'telegram'"
          v-model="form.telegramToken"
          :label="t('channel.botToken')"
          type="password"
          placeholder="123456:ABC-DEF..."
          class="mt-4"
        />
      </GlassCard>

      <!-- Step 3: Security (optional) -->
      <GlassCard v-if="step === 2" :hoverable="false">
        <p class="step-hint">{{ t('setup.securityHint') }}</p>
        <div class="preset-grid">
          <button
            v-for="preset in presets"
            :key="preset.value"
            class="preset-card"
            :class="{ 'preset-card--active': form.autonomy === preset.value }"
            @click="form.autonomy = preset.value"
          >
            <div class="preset-card__title">{{ preset.label }}</div>
            <div class="preset-card__desc">{{ preset.desc }}</div>
          </button>
        </div>
      </GlassCard>

      <!-- Navigation -->
      <div class="flex justify-between mt-6">
        <GlassButton v-if="step > 0" variant="ghost" @click="step--">{{ t('common.back') }}</GlassButton>
        <div v-else />
        <div class="flex gap-2">
          <GlassButton v-if="step === 1 || step === 2" variant="ghost" @click="step++">
            {{ t('common.skip') }}
          </GlassButton>
          <GlassButton
            v-if="step < 2"
            variant="primary"
            :disabled="step === 0 && !form.apiKey"
            @click="step++"
          >
            {{ t('common.next') }}
          </GlassButton>
          <GlassButton v-else variant="primary" :loading="saving" @click="finish">
            {{ t('setup.saveAndStart') }}
          </GlassButton>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed } from 'vue'
import { useRouter } from 'vue-router'
import { GlassCard, GlassButton, GlassInput, GlassSelect, GlassSteps } from '../components/glass'
import { writeConfig, startPlaw, testProviderConnection } from '../api/tauri'
import { useI18n } from '../composables/useI18n'

const { t } = useI18n()
const router = useRouter()
const step = ref(0)
const saving = ref(false)
const testing = ref(false)
const testMsg = ref('')
const testOk = ref(false)

const form = ref({
  provider: 'kimi-coder',
  apiKey: '',
  baseUrl: '',
  model: '',
  channel: 'none',
  telegramToken: '',
  autonomy: 'supervised',
})

const providerOptions = [
  { label: 'Kimi Coder K2.5 (Default)', value: 'kimi-coder' },
  { label: 'Kimi K2.5 (Moonshot)', value: 'kimi-moonshot' },
  { label: 'Anthropic (Claude)', value: 'anthropic' },
  { label: 'OpenAI', value: 'openai' },
  { label: 'OpenRouter', value: 'openrouter' },
  { label: 'Ollama (Local)', value: 'ollama' },
  { label: 'Custom URL', value: 'custom' },
]

const channelOptions = computed(() => [
  { label: t('setup.channelNone'), value: 'none' },
  { label: 'Telegram', value: 'telegram' },
  { label: 'Discord', value: 'discord' },
  { label: 'Slack', value: 'slack' },
])

const presets = computed(() => [
  { value: 'readonly', label: t('security.conservative'), desc: t('security.conservativeDesc') },
  { value: 'supervised', label: t('security.standard'), desc: t('security.standardDesc') },
  { value: 'full', label: t('security.permissive'), desc: t('security.permissiveDesc') },
])

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
const modelOpts = computed(() => MODELS[form.value.provider] || [])

async function testConnection() {
  testing.value = true
  testMsg.value = t('provider.testing')
  testOk.value = false
  try {
    await testProviderConnection(form.value.provider, form.value.apiKey, form.value.baseUrl, form.value.model)
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

async function finish() {
  saving.value = true
  try {
    const KIMI_PROVIDERS = {
      'kimi-coder': { url: 'anthropic-custom:https://api.kimi.com/coding', model: 'k2p5' },
      'kimi-moonshot': { url: 'anthropic-custom:https://api.moonshot.cn', model: 'kimi-k2.5' },
    }
    const kimiCfg = KIMI_PROVIDERS[form.value.provider]
    const cfg = {
      default_provider: kimiCfg ? kimiCfg.url : form.value.provider,
      api_key: form.value.apiKey,
      default_model: kimiCfg ? kimiCfg.model : form.value.model,
      default_temperature: 0.7,
    }
    if (kimiCfg) {
      cfg.provider = { reasoning_level: 'medium' }
    }
    if (!kimiCfg && form.value.baseUrl) cfg.provider_api = form.value.baseUrl
    if (form.value.channel !== 'none' && form.value.telegramToken) {
      cfg.telegram = { bot_token: form.value.telegramToken }
    }
    cfg.gateway = {
      require_pairing: false,
    }
    cfg.autonomy = {
      level: form.value.autonomy,
      workspace_only: form.value.autonomy !== 'full',
      allowed_commands: [],
      forbidden_paths: [],
      max_actions_per_hour: 1000,
      max_cost_per_day_cents: 10000,
    }
    // Tool configs — enable web search and fetch by default
    cfg.web_search = {
      enabled: true,
      provider: 'bing',
      max_results: 5,
      timeout_secs: 15,
    }
    cfg.web_fetch = {
      enabled: true,
      provider: 'fast_html2md',
      allowed_domains: ['*'],
      max_response_size: 524288,
      timeout_secs: 30,
    }
    cfg.http_request = {
      enabled: true,
      allowed_domains: ['localhost', '127.0.0.1'],
      allow_local: true,
      max_response_size: 1048576,
      timeout_secs: 120,
    }
    cfg.browser = {
      ...cfg.browser,
      enabled: true,
      allowed_domains: ['*'],
    }
    await writeConfig(cfg)
    try { await startPlaw() } catch { /* ignore start error */ }
    router.push('/')
  } finally { saving.value = false }
}
</script>

<style scoped>
.setup-root {
  width: 100%;
  height: 100vh;
  display: flex; align-items: center; justify-content: center;
  background: var(--bg-base);
}
.setup-container { width: 100%; max-width: 520px; padding: 2rem; }

.setup-brand {
  display: flex; align-items: center; justify-content: center;
  gap: 12px; margin-bottom: 8px;
}
.setup-brand__logo {
  width: 44px; height: 44px;
  display: flex; align-items: center; justify-content: center;
  background: var(--lobster-primary);
  color: white;
  font-size: 1.4rem; font-weight: 800;
  border-radius: var(--radius-md);
}
.setup-brand__title {
  font-size: 1.6rem; font-weight: 700;
  color: var(--text-primary);
  letter-spacing: -0.02em;
}
.setup-subtitle {
  text-align: center;
  color: var(--text-muted);
  font-size: 0.9rem;
  margin-bottom: 32px;
}

.step-hint {
  color: var(--text-secondary);
  font-size: 0.875rem;
  margin-bottom: 16px;
}

.preset-grid {
  display: grid; grid-template-columns: repeat(3, 1fr);
  gap: 12px;
}
.preset-card {
  background: var(--bg-raised);
  border: 2px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: 1rem; text-align: center;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
  color: var(--text-primary);
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
.test-msg { font-size: 0.8rem; font-weight: 500; }
.test-msg--ok { color: var(--status-ok); }
.test-msg--err { color: var(--status-err); }
</style>
