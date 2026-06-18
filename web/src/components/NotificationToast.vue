<template>
  <Teleport to="body">
    <TransitionGroup name="toast" tag="div" class="toast-container">
      <div
        v-for="toast in toasts"
        :key="toast.id"
        class="toast-item"
        :class="`toast-item--${toast.type}`"
        @click="handleClick(toast)"
      >
        <div class="toast-item__header">
          <span class="toast-item__icon">{{ iconFor(toast.type) }}</span>
          <span class="toast-item__title">{{ toast.title }}</span>
          <button class="toast-item__close" @click.stop="dismissToast(toast.id)">&times;</button>
        </div>
        <div v-if="toast.body" class="toast-item__body">{{ toast.body }}</div>
      </div>
    </TransitionGroup>
  </Teleport>
</template>

<script setup>
import { useNotifications } from '../composables/useNotifications'

const { toasts, dismissToast, handleClick } = useNotifications()

function iconFor(type) {
  if (type === 'success') return '\u2705'
  if (type === 'error') return '\u274C'
  return '\uD83D\uDD14'
}
</script>

<style scoped>
.toast-container {
  position: fixed;
  top: 16px;
  right: 16px;
  z-index: var(--z-notification);
  display: flex;
  flex-direction: column;
  gap: 10px;
  pointer-events: none;
  max-width: 380px;
}

.toast-item {
  pointer-events: auto;
  cursor: pointer;
  background: var(--bg-overlay);
  backdrop-filter: blur(var(--blur-lg));
  -webkit-backdrop-filter: blur(var(--blur-lg));
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  padding: 14px 16px;
  box-shadow: var(--shadow-card-hover);
  transition: transform var(--duration-fast) var(--ease-out),
              box-shadow var(--duration-fast) var(--ease-out);
}
.toast-item:hover {
  transform: translateX(-4px);
  box-shadow: var(--shadow-glow), var(--shadow-card-hover);
}

.toast-item--success { border-left: 3px solid var(--status-ok); }
.toast-item--error { border-left: 3px solid var(--status-err); }
.toast-item--info { border-left: 3px solid var(--plaw-primary); }

.toast-item__header {
  display: flex;
  align-items: center;
  gap: 8px;
}
.toast-item__icon { font-size: 0.9rem; flex-shrink: 0; }
.toast-item__title {
  flex: 1;
  font-size: 0.84rem;
  font-weight: 600;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.toast-item__close {
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 1.1rem;
  cursor: pointer;
  padding: 0 4px;
  line-height: 1;
  border-radius: var(--radius-sm);
  transition: color var(--duration-fast) var(--ease-out),
              background var(--duration-fast) var(--ease-out);
}
.toast-item__close:hover {
  color: var(--text-primary);
  background: var(--plaw-primary-soft);
}

.toast-item__body {
  margin-top: 6px;
  font-size: 0.78rem;
  color: var(--text-secondary);
  line-height: 1.5;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

/* Transition */
.toast-enter-active { transition: all var(--duration-normal) var(--ease-out); }
.toast-leave-active { transition: all 0.2s ease-in; }
.toast-enter-from { opacity: 0; transform: translateX(80px) scale(0.95); }
.toast-leave-to { opacity: 0; transform: translateX(80px) scale(0.95); }
.toast-move { transition: transform var(--duration-normal) var(--ease-out); }
</style>
