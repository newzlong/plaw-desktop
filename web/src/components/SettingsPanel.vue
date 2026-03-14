<template>
  <Teleport to="body">
    <transition name="settings-fade">
      <div v-if="modelValue" class="settings-overlay">
        <div class="settings-panel">
          <div class="settings-panel__header">
            <h2>{{ t('settings.title') }}</h2>
            <button class="settings-panel__close" @click="$emit('update:modelValue', false)">×</button>
          </div>

          <div class="settings-panel__body">
            <!-- Tab navigation -->
            <nav class="settings-tabs">
              <button
                v-for="tab in tabs"
                :key="tab.id"
                class="settings-tab"
                :class="{ 'settings-tab--active': activeTab === tab.id }"
                @click="activeTab = tab.id"
              >
                <component :is="tab.icon" class="w-4 h-4" />
                <span>{{ t(tab.i18nKey) }}</span>
              </button>
            </nav>

            <!-- Tab content -->
            <div class="settings-content">
              <KeepAlive>
                <component :is="activeComponent" />
              </KeepAlive>
            </div>
          </div>
        </div>
      </div>
    </transition>
  </Teleport>
</template>

<script setup>
import { ref, computed } from 'vue'
import { useI18n } from '../composables/useI18n'
import {
  LayoutDashboard, Bot, Radio, Shield,
  Puzzle, Users, Clock, BookOpen, Pill, FileText,
} from 'lucide-vue-next'

import { defineAsyncComponent } from 'vue'

const Dashboard = defineAsyncComponent(() => import('../views/Dashboard.vue'))
const ProviderConfig = defineAsyncComponent(() => import('../views/ProviderConfig.vue'))
const ChannelConfig = defineAsyncComponent(() => import('../views/ChannelConfig.vue'))
const SecurityConfig = defineAsyncComponent(() => import('../views/SecurityConfig.vue'))
const SkillsManager = defineAsyncComponent(() => import('../views/SkillsManager.vue'))
const AgentManager = defineAsyncComponent(() => import('../views/AgentManager.vue'))
const CronManager = defineAsyncComponent(() => import('../views/CronManager.vue'))
const KnowledgeManager = defineAsyncComponent(() => import('../views/KnowledgeManager.vue'))
const CapsulesPanel = defineAsyncComponent(() => import('../views/CapsulesPanel.vue'))
const Logs = defineAsyncComponent(() => import('../views/Logs.vue'))

defineProps({ modelValue: Boolean })
defineEmits(['update:modelValue'])

const { t } = useI18n()

const tabs = [
  { id: 'dashboard', i18nKey: 'nav.dashboard', icon: LayoutDashboard },
  { id: 'provider', i18nKey: 'nav.provider', icon: Bot },
  { id: 'channel', i18nKey: 'nav.channel', icon: Radio },
  { id: 'security', i18nKey: 'nav.security', icon: Shield },
  { id: 'skills', i18nKey: 'nav.skills', icon: Puzzle },
  { id: 'agents', i18nKey: 'nav.agents', icon: Users },
  { id: 'cron', i18nKey: 'nav.cron', icon: Clock },
  { id: 'knowledge', i18nKey: 'nav.knowledge', icon: BookOpen },
  { id: 'capsules', i18nKey: 'nav.capsules', icon: Pill },
  { id: 'logs', i18nKey: 'nav.logs', icon: FileText },
]

const activeTab = ref('dashboard')

// Expose openTab for external callers
function openTab(tabId) {
  if (componentMap[tabId]) {
    activeTab.value = tabId
  }
}
defineExpose({ openTab })

const componentMap = {
  dashboard: Dashboard,
  provider: ProviderConfig,
  channel: ChannelConfig,
  security: SecurityConfig,
  skills: SkillsManager,
  agents: AgentManager,
  cron: CronManager,
  knowledge: KnowledgeManager,
  capsules: CapsulesPanel,
  logs: Logs,
}

const activeComponent = computed(() => componentMap[activeTab.value])
</script>

<style scoped>
.settings-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(0, 0, 0, 0.5);
  backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
}

.settings-panel {
  width: min(920px, 92vw);
  height: min(680px, 88vh);
  background: var(--bg-overlay);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 24px 80px rgba(0, 0, 0, 0.35);
}

.settings-panel__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.settings-panel__header h2 {
  font-size: 1.05rem;
  font-weight: 700;
  margin: 0;
}
.settings-panel__close {
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  font-size: 1.1rem;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.settings-panel__close:hover {
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
  border-color: var(--plaw-primary-soft);
}

.settings-panel__body {
  flex: 1;
  display: flex;
  overflow: hidden;
}

/* --- Tab navigation --- */
.settings-tabs {
  width: 160px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 12px 8px;
  border-right: 1px solid var(--border-subtle);
  overflow-y: auto;
}

.settings-tab {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: var(--radius-sm);
  font-size: 0.82rem;
  font-weight: 500;
  color: var(--text-secondary);
  background: transparent;
  border: none;
  cursor: pointer;
  text-align: left;
  transition: all var(--duration-fast) var(--ease-out);
}
.settings-tab:hover {
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
}
.settings-tab--active {
  background: var(--plaw-primary-soft) !important;
  color: var(--plaw-primary) !important;
  font-weight: 600;
}

/* --- Tab content --- */
.settings-content {
  flex: 1;
  overflow-y: auto;
  padding: 20px 24px;
}

/* === Override child view styles for embedded mode === */
.settings-content :deep(> div) {
  padding: 0 !important;
  max-width: none !important;
}
/* Remove Tailwind max-width constraints inside settings */
.settings-content :deep(.max-w-xl),
.settings-content :deep(.max-w-2xl),
.settings-content :deep(.max-w-3xl) {
  max-width: none !important;
}

/* === Kill inner scroll containers — settings-content is the ONLY scroller === */
.settings-content :deep(.log-container) {
  max-height: none !important;
  overflow-y: visible !important;
}

/* === Sticky page headers (Skills, Agents, Cron, Knowledge) === */
.settings-content :deep(.page-header) {
  position: sticky;
  top: -20px;              /* counteract parent padding */
  z-index: 10;
  background: var(--bg-overlay);
  margin: -20px -24px 16px; /* bleed into parent padding */
  padding: 20px 24px 12px;
}

/* === Sticky page title for simple views (Provider, Security, Channel, Logs) === */
.settings-content :deep(.page-title) {
  font-size: 1.05rem;
  margin-bottom: 12px;
}
.settings-content :deep(.page-desc) {
  font-size: 0.82rem;
  margin-bottom: 16px;
}

/* === Sticky tabs (Skills installed/market) === */
.settings-content :deep(.tabs) {
  position: sticky;
  top: 48px;               /* below the sticky page-header */
  z-index: 9;
  background: var(--bg-overlay);
  margin-left: -24px;
  margin-right: -24px;
  padding-left: 24px;
  padding-right: 24px;
  padding-bottom: 4px;
}

/* === Sticky save/action bars at bottom === */
.settings-content :deep(.sticky-actions) {
  position: sticky;
  bottom: -20px;           /* counteract parent padding */
  z-index: 10;
  background: var(--bg-overlay);
  margin: 0 -24px -20px;
  padding: 12px 24px 20px;
  border-top: 1px solid var(--border-subtle);
}

/* === Hero status in dashboard: fit within panel + sticky === */
.settings-content :deep(.hero-status) {
  position: sticky;
  top: -20px;
  z-index: 10;
  margin: -20px -24px 16px;
  padding: 20px 24px;
  border-radius: 0;
}

/* --- Transition --- */
.settings-fade-enter-active,
.settings-fade-leave-active {
  transition: opacity var(--duration-normal) var(--ease-out);
}
.settings-fade-enter-active .settings-panel,
.settings-fade-leave-active .settings-panel {
  transition: transform var(--duration-normal) var(--ease-out),
              opacity var(--duration-normal) var(--ease-out);
}
.settings-fade-enter-from,
.settings-fade-leave-to {
  opacity: 0;
}
.settings-fade-enter-from .settings-panel {
  transform: scale(0.95) translateY(10px);
  opacity: 0;
}
.settings-fade-leave-to .settings-panel {
  transform: scale(0.98) translateY(5px);
  opacity: 0;
}
</style>
