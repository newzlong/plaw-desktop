<template>
  <div class="chat-sidebar" :class="{ 'chat-sidebar--collapsed': sidebarCollapsed }">
    <div class="sidebar-topbar">
      <button class="sidebar-toggle" :title="sidebarCollapsed ? (isZh ? '展开侧边栏' : 'Expand sidebar') : (isZh ? '收起侧边栏' : 'Collapse sidebar')" @click="$emit('toggle-sidebar')">
        <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
          <rect x="1" y="2" width="12" height="1.5" rx="0.75" fill="currentColor"/>
          <rect x="1" y="6.25" width="12" height="1.5" rx="0.75" fill="currentColor"/>
          <rect x="1" y="10.5" width="12" height="1.5" rx="0.75" fill="currentColor"/>
        </svg>
      </button>
      <button v-if="!sidebarCollapsed" class="sidebar-new" :disabled="streaming" @click="$emit('new-session')">
        <span>+</span> {{ t('chat.newChat') }}
      </button>
    </div>
    <div class="sidebar-list">
      <div v-if="!sessions.length" class="sidebar-empty">
        {{ t('chat.noHistory') }}
      </div>
      <div
        v-for="s in sessions"
        :key="s.id"
        class="sidebar-item"
        :class="{ 'sidebar-item--active': s.id === currentSessionId, 'sidebar-item--disabled': streaming && s.id !== currentSessionId }"
        @click="!streaming && $emit('load-session', s.id)"
      >
        <div class="sidebar-item__title">{{ s.title || t('chat.untitled') }}</div>
        <div class="sidebar-item__meta">{{ s.message_count }} {{ t('chat.placeholder').split(' ')[0] === '输入' ? '条' : 'msgs' }}</div>
        <button v-if="!streaming" class="sidebar-item__delete" @click.stop="$emit('remove-session', s.id)">×</button>
      </div>
    </div>

    <!-- Sidebar footer: plaw + settings + theme + language -->
    <div class="sidebar-footer">
      <button
        class="sidebar-footer__btn sidebar-footer__zc"
        :class="{
          'sidebar-footer__zc--running': canStop,
          'sidebar-footer__zc--stopped': canStart,
          'sidebar-footer__zc--busy': isBusy,
        }"
        :disabled="isBusy"
        :title="zcState"
        @click="togglePlaw"
      >
        <Loader2 v-if="isBusy" class="w-4 h-4 spin-icon" />
        <Square v-else-if="canStop" class="w-3.5 h-3.5" />
        <Play v-else class="w-4 h-4" />
      </button>
    </div>
  </div>
</template>

<script setup>
import { Play, Square, Loader2 } from 'lucide-vue-next'
import { usePlawState } from '../composables/usePlawState'
import { useI18n } from '../composables/useI18n'
import { startPlaw, stopPlaw } from '../api/tauri'

const props = defineProps({
  sessions: {
    type: Array,
    default: () => [],
  },
  currentSessionId: {
    type: String,
    default: null,
  },
  streaming: {
    type: Boolean,
    default: false,
  },
  sidebarCollapsed: {
    type: Boolean,
    default: false,
  },
})

const emit = defineEmits(['new-session', 'load-session', 'remove-session', 'toggle-sidebar'])

const { t, isZh } = useI18n()
const { state: zcState, canStart, canStop, isBusy } = usePlawState()

async function togglePlaw() {
  if (isBusy.value) return
  try {
    if (canStop.value) await stopPlaw()
    else if (canStart.value) await startPlaw()
  } catch {}
}
</script>

<style scoped>
/* ---- Sidebar ---- */
.chat-sidebar {
  width: 220px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  margin: 8px 0 8px 8px;
  background: var(--sidebar-bg);
  border: 1px solid var(--sidebar-border);
  border-radius: var(--radius-lg);
  overflow: hidden;
  transition: width var(--duration-normal) var(--ease-out),
              min-width var(--duration-normal) var(--ease-out);
}

/* Collapsed sidebar: icon-only strip */
.chat-sidebar--collapsed {
  width: 48px;
  min-width: 48px;
}
.chat-sidebar--collapsed .sidebar-list,
.chat-sidebar--collapsed .sidebar-empty {
  display: none;
}

/* Sidebar topbar: toggle button + new-chat button on same row */
.sidebar-topbar {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 8px 8px 0;
  flex-shrink: 0;
}

.sidebar-toggle {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  background: transparent;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-toggle:hover {
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
  border-color: var(--plaw-primary-soft);
}

/* New-chat button inside topbar: stretch to fill remaining space */
.sidebar-topbar .sidebar-new {
  flex: 1;
  margin: 0;
  min-width: 0;
}

/* Collapsed state: topbar centers the toggle button */
.chat-sidebar--collapsed .sidebar-topbar {
  justify-content: center;
  margin: 8px 0;
}

.sidebar-new {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 8px 8px 0;
  padding: 10px 14px;
  background: var(--plaw-primary-soft);
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  color: var(--plaw-primary);
  font-size: 0.84rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-new:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.sidebar-new:not(:disabled):hover {
  background: var(--plaw-primary);
  color: white;
  border-color: var(--plaw-primary);
}
.sidebar-new span {
  font-size: 1.1rem;
}

.sidebar-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px 8px;
}

.sidebar-empty {
  padding: 20px 16px;
  color: var(--text-muted);
  font-size: 0.8rem;
  text-align: center;
}

.sidebar-item {
  position: relative;
  padding: 10px 12px;
  padding-left: 9px;
  margin-bottom: 2px;
  cursor: pointer;
  border-radius: var(--radius-sm);
  border-left: 3px solid transparent;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-item:not(.sidebar-item--disabled):hover {
  background: var(--bg-surface-hover);
}
.sidebar-item--disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.sidebar-item--active {
  background: var(--plaw-primary-soft);
  border-left-color: var(--plaw-primary);
}

.sidebar-item__title {
  font-size: 0.82rem;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  padding-right: 20px;
  transition: color var(--duration-fast) var(--ease-out);
}
.sidebar-item--active .sidebar-item__title {
  color: var(--plaw-primary);
  font-weight: 600;
}
.sidebar-item__meta {
  font-size: 0.72rem;
  color: var(--text-muted);
  margin-top: 3px;
}
.sidebar-item__delete {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-sm);
  border: none;
  background: none;
  color: var(--text-muted);
  font-size: 0.9rem;
  cursor: pointer;
  opacity: 0;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-item:hover .sidebar-item__delete {
  opacity: 1;
}
.sidebar-item__delete:hover {
  color: var(--status-err);
  background: var(--plaw-primary-soft);
}

/* ---- Sidebar footer ---- */
.sidebar-footer {
  display: flex;
  justify-content: center;
  gap: 6px;
  padding: 8px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.sidebar-footer__btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background: var(--input-bg);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-footer__btn:hover {
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
  border-color: var(--plaw-primary-soft);
}

/* Plaw toggle button */
.sidebar-footer__zc--running {
  color: var(--status-ok) !important;
  border-color: rgba(34, 197, 94, 0.3);
}
.sidebar-footer__zc--running:hover {
  background: rgba(239, 68, 68, 0.12) !important;
  color: var(--status-err) !important;
  border-color: rgba(239, 68, 68, 0.3) !important;
}
.sidebar-footer__zc--stopped {
  color: var(--text-muted) !important;
}
.sidebar-footer__zc--stopped:hover {
  background: rgba(34, 197, 94, 0.12) !important;
  color: var(--status-ok) !important;
  border-color: rgba(34, 197, 94, 0.3) !important;
}
.sidebar-footer__zc--busy {
  opacity: 0.6;
  cursor: not-allowed !important;
}
.spin-icon {
  animation: spin-zc 1s linear infinite;
}
@keyframes spin-zc {
  to { transform: rotate(360deg); }
}

/* ---- Responsive breakpoints ---- */
@media (max-width: 768px) {
  .chat-sidebar {
    /* On small windows, sidebar shrinks to icon-only automatically via JS.
       But even without JS, constrain max width so chat area stays usable. */
    width: 48px;
    min-width: 48px;
  }
  .chat-sidebar:not(.chat-sidebar--collapsed) {
    /* User manually expanded on small screen: allow it but limit width */
    width: 200px;
    min-width: 200px;
  }
}
</style>
