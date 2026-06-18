<template>
  <div>
    <h1 class="page-title">{{ t('channel.title') }}</h1>
    <p class="page-desc">{{ t('channel.desc') }}</p>

    <div class="max-w-xl space-y-4">
      <!-- Telegram -->
      <GlassCard :hoverable="false">
        <div class="channel-header" role="button" tabindex="0" @click="expanded.telegram = !expanded.telegram" @keydown.enter="expanded.telegram = !expanded.telegram" :aria-expanded="expanded.telegram">
          <div class="channel-header__left">
            <MessageCircle class="w-5 h-5" style="color: var(--channel-telegram)" />
            <span class="channel-header__name">Telegram</span>
          </div>
          <div class="channel-header__right">
            <GlassToggle v-model="form.telegram.enabled" @click.stop />
            <ChevronDown class="w-4 h-4 expand-icon" :class="{ 'expand-icon--open': expanded.telegram }" />
          </div>
        </div>
        <div v-if="expanded.telegram" class="channel-body">
          <GlassInput
            v-model="form.telegram.bot_token"
            :label="t('channel.botToken')"
            type="password"
            placeholder="123456:ABC-DEF..."
            hint="From @BotFather on Telegram"
          />
          <GlassInput
            v-model="form.telegram.allowed_users"
            :label="t('channel.allowedUsers')"
            placeholder="123456789, 987654321"
            class="mt-3"
          />
        </div>
      </GlassCard>

      <!-- Discord -->
      <GlassCard :hoverable="false">
        <div class="channel-header" role="button" tabindex="0" @click="expanded.discord = !expanded.discord" @keydown.enter="expanded.discord = !expanded.discord" :aria-expanded="expanded.discord">
          <div class="channel-header__left">
            <Hash class="w-5 h-5" style="color: var(--channel-discord)" />
            <span class="channel-header__name">Discord</span>
          </div>
          <div class="channel-header__right">
            <GlassToggle v-model="form.discord.enabled" @click.stop />
            <ChevronDown class="w-4 h-4 expand-icon" :class="{ 'expand-icon--open': expanded.discord }" />
          </div>
        </div>
        <div v-if="expanded.discord" class="channel-body">
          <GlassInput
            v-model="form.discord.bot_token"
            :label="t('channel.botToken')"
            type="password"
            placeholder="Discord bot token..."
          />
          <GlassInput
            v-model="form.discord.guild_id"
            :label="t('channel.guildId')"
            placeholder="Server ID"
            class="mt-3"
          />
          <GlassInput
            v-model="form.discord.channel_id"
            :label="t('channel.channelId')"
            placeholder="Channel ID"
            class="mt-3"
          />
        </div>
      </GlassCard>

      <!-- Slack -->
      <GlassCard :hoverable="false">
        <div class="channel-header" role="button" tabindex="0" @click="expanded.slack = !expanded.slack" @keydown.enter="expanded.slack = !expanded.slack" :aria-expanded="expanded.slack">
          <div class="channel-header__left">
            <AtSign class="w-5 h-5" style="color: var(--channel-slack)" />
            <span class="channel-header__name">Slack</span>
          </div>
          <div class="channel-header__right">
            <GlassToggle v-model="form.slack.enabled" @click.stop />
            <ChevronDown class="w-4 h-4 expand-icon" :class="{ 'expand-icon--open': expanded.slack }" />
          </div>
        </div>
        <div v-if="expanded.slack" class="channel-body">
          <GlassInput
            v-model="form.slack.bot_token"
            :label="t('channel.botToken')"
            type="password"
            placeholder="xoxb-..."
          />
          <GlassInput
            v-model="form.slack.app_token"
            :label="t('channel.appToken')"
            type="password"
            placeholder="xapp-..."
            class="mt-3"
          />
          <GlassInput
            v-model="form.slack.channel"
            :label="t('channel.channelName')"
            placeholder="#general"
            class="mt-3"
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
import { MessageCircle, Hash, AtSign, ChevronDown } from 'lucide-vue-next'
import { GlassCard, GlassButton, GlassInput, GlassToggle } from '../components/glass'
import { readConfig, writeConfig, restartPlaw, getPlawStatus } from '../api/tauri'
import { useI18n } from '../composables/useI18n'
const { t } = useI18n()

const expanded = reactive({ telegram: false, discord: false, slack: false })
const saving = ref(false)
const saveMsg = ref('')
const saveOk = ref(false)
const needRestart = ref(false)
const restarting = ref(false)

const form = reactive({
  telegram: { enabled: false, bot_token: '', allowed_users: '' },
  discord: { enabled: false, bot_token: '', guild_id: '', channel_id: '' },
  slack: { enabled: false, bot_token: '', app_token: '', channel: '' },
})

onMounted(async () => {
  try {
    const cfg = await readConfig()
    if (cfg.telegram) {
      form.telegram.enabled = true
      form.telegram.bot_token = cfg.telegram.bot_token || ''
      form.telegram.allowed_users = (cfg.telegram.allowed_users || []).join(', ')
      expanded.telegram = true
    }
    if (cfg.discord) {
      form.discord.enabled = true
      form.discord.bot_token = cfg.discord.bot_token || ''
      form.discord.guild_id = cfg.discord.guild_id || ''
      form.discord.channel_id = cfg.discord.channel_id || ''
      expanded.discord = true
    }
    if (cfg.slack) {
      form.slack.enabled = true
      form.slack.bot_token = cfg.slack.bot_token || ''
      form.slack.app_token = cfg.slack.app_token || ''
      form.slack.channel = cfg.slack.channel || ''
      expanded.slack = true
    }
  } catch { /* no config yet */ }
})

async function save() {
  saving.value = true
  try {
    const cfg = {}
    try { Object.assign(cfg, await readConfig()) } catch {}

    // Remove old channel configs first
    delete cfg.telegram
    delete cfg.discord
    delete cfg.slack

    if (form.telegram.enabled && form.telegram.bot_token) {
      cfg.telegram = { bot_token: form.telegram.bot_token }
      const users = form.telegram.allowed_users.split(',').map(s => s.trim()).filter(Boolean)
      if (users.length) cfg.telegram.allowed_users = users
    }
    if (form.discord.enabled && form.discord.bot_token) {
      cfg.discord = {
        bot_token: form.discord.bot_token,
        guild_id: form.discord.guild_id || undefined,
        channel_id: form.discord.channel_id || undefined,
      }
    }
    if (form.slack.enabled && form.slack.bot_token) {
      cfg.slack = {
        bot_token: form.slack.bot_token,
        app_token: form.slack.app_token || undefined,
        channel: form.slack.channel || undefined,
      }
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

.channel-header {
  display: flex; align-items: center; justify-content: space-between;
  cursor: pointer; user-select: none;
}
.channel-header__left {
  display: flex; align-items: center; gap: 10px;
}
.channel-header__name {
  font-size: 0.95rem; font-weight: 600;
  color: var(--text-primary);
}
.channel-header__right {
  display: flex; align-items: center; gap: 10px;
}
.expand-icon {
  color: var(--text-muted);
  transition: transform var(--duration-fast) var(--ease-out);
}
.expand-icon--open { transform: rotate(180deg); }

.channel-body {
  margin-top: 16px;
  padding-top: 16px;
  border-top: 1px solid var(--border-subtle);
}

.save-msg { font-size: 0.82rem; font-weight: 500; transition: opacity 0.3s; }
.save-msg--ok { color: var(--status-ok); }
.save-msg--err { color: var(--status-err); }
.restart-bar {
  display: flex; align-items: center; justify-content: space-between;
  background: var(--plaw-primary-soft);
  border: 1px solid var(--plaw-primary);
  border-radius: var(--radius-md);
  padding: 10px 16px;
  font-size: 0.85rem;
  color: var(--text-primary);
}
</style>
