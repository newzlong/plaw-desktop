<template>
  <Teleport to="body">
    <TransitionGroup name="toast" tag="div" class="glass-toast__container">
      <div
        v-for="t in toasts"
        :key="t.id"
        class="glass-toast"
        :class="`glass-toast--${t.type}`"
        role="alert"
      >
        <span class="glass-toast__msg">{{ t.message }}</span>
        <button class="glass-toast__close" @click="remove(t.id)">&times;</button>
      </div>
    </TransitionGroup>
  </Teleport>
</template>

<script setup>
import { ref } from 'vue'

const toasts = ref([])
let nextId = 0

function add(message, type = 'info', duration = 3000) {
  const id = nextId++
  toasts.value.push({ id, message, type })
  if (duration > 0) {
    setTimeout(() => remove(id), duration)
  }
}

function remove(id) {
  toasts.value = toasts.value.filter(t => t.id !== id)
}

defineExpose({
  add, remove,
  success: (m) => add(m, 'success'),
  error: (m) => add(m, 'error'),
  warning: (m) => add(m, 'warning'),
})
</script>

<style scoped>
.glass-toast__container {
  position: fixed; top: 16px; right: 16px; z-index: 200;
  display: flex; flex-direction: column; gap: 8px;
  pointer-events: none;
}
.glass-toast {
  pointer-events: auto;
  display: flex; align-items: center; gap: 0.5rem;
  padding: 0.65rem 1rem;
  background: var(--bg-overlay);
  backdrop-filter: blur(12px);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: 0.85rem;
  box-shadow: 0 8px 24px rgba(0,0,0,0.2);
  min-width: 240px; max-width: 400px;
}
.glass-toast--success { border-left: 3px solid var(--status-ok); }
.glass-toast--error { border-left: 3px solid var(--status-err); }
.glass-toast--warning { border-left: 3px solid var(--status-warn); }
.glass-toast--info { border-left: 3px solid var(--plaw-primary); }
.glass-toast__msg { flex: 1; }
.glass-toast__close {
  background: none; border: none;
  color: var(--text-muted);
  cursor: pointer; font-size: 1.1rem;
  padding: 0; line-height: 1;
  transition: color var(--duration-fast);
}
.glass-toast__close:hover { color: var(--plaw-primary); }
.toast-enter-active, .toast-leave-active {
  transition: all var(--duration-normal) var(--ease-out);
}
.toast-enter-from { opacity: 0; transform: translateX(20px); }
.toast-leave-to { opacity: 0; transform: translateX(20px); }
</style>
