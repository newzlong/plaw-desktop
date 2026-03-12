<template>
  <button
    class="glass-btn"
    :class="[
      `glass-btn--${variant}`,
      { 'glass-btn--sm': size === 'sm', 'glass-btn--lg': size === 'lg' },
    ]"
    :disabled="disabled || loading"
  >
    <span v-if="loading" class="glass-btn__spinner" />
    <slot />
  </button>
</template>

<script setup>
defineProps({
  variant: { type: String, default: 'default', validator: v => ['default', 'primary', 'danger', 'ghost'].includes(v) },
  size: { type: String, default: 'md' },
  disabled: { type: Boolean, default: false },
  loading: { type: Boolean, default: false },
})
</script>

<style scoped>
.glass-btn {
  display: inline-flex;
  align-items: center; justify-content: center;
  gap: 0.5rem;
  background: var(--bg-surface);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  padding: 0.5rem 1.25rem;
  color: var(--text-primary);
  font-size: 0.875rem; font-weight: 500;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
  white-space: nowrap;
}
.glass-btn:hover:not(:disabled) {
  background: var(--bg-surface-hover);
  border-color: var(--border-strong);
}
.glass-btn:focus-visible {
  outline: none;
  box-shadow: var(--input-focus-shadow);
}
.glass-btn:disabled {
  opacity: 0.45;
  cursor: not-allowed;
}

/* Primary */
.glass-btn--primary {
  background: var(--plaw-primary);
  border-color: var(--plaw-primary);
  color: white;
}
.glass-btn--primary:hover:not(:disabled) {
  background: var(--plaw-primary-hover);
  border-color: var(--plaw-primary-hover);
  box-shadow: var(--shadow-glow);
}

/* Danger */
.glass-btn--danger {
  background: var(--status-err);
  border-color: var(--status-err);
  color: white;
}
.glass-btn--danger:hover:not(:disabled) {
  filter: brightness(1.1);
}

/* Ghost */
.glass-btn--ghost {
  background: transparent;
  border-color: transparent;
  color: var(--text-secondary);
}
.glass-btn--ghost:hover:not(:disabled) {
  background: var(--plaw-primary-soft);
  color: var(--plaw-primary);
}

/* Sizes */
.glass-btn--sm {
  padding: 0.3rem 0.75rem;
  font-size: 0.78rem;
  border-radius: 6px;
}
.glass-btn--lg {
  padding: 0.7rem 1.75rem;
  font-size: 1rem;
}

/* Spinner */
.glass-btn__spinner {
  width: 14px; height: 14px;
  border: 2px solid rgba(255,255,255,0.3);
  border-top-color: currentColor;
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }
</style>
