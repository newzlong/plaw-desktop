<template>
  <Teleport to="body">
    <Transition name="dialog">
      <div
        v-if="modelValue"
        class="glass-dialog__overlay"
        role="dialog"
        aria-modal="true"
        @click.self="!persistent && closable && close()"
        @keydown.escape="closable && close()"
      >
        <div class="glass-dialog__panel" :style="{ maxWidth: width }">
          <div class="glass-dialog__header">
            <h3 class="glass-dialog__title">{{ title }}</h3>
            <button v-if="closable" class="glass-dialog__close" @click="close" aria-label="Close">&times;</button>
          </div>
          <div class="glass-dialog__body">
            <slot>
              <p v-if="message" class="glass-dialog__message">{{ message }}</p>
            </slot>
          </div>
          <div v-if="$slots.footer || message" class="glass-dialog__footer">
            <slot name="footer">
              <GlassButton @click="close">{{ cancelText || 'Cancel' }}</GlassButton>
              <GlassButton :variant="variant === 'danger' ? 'danger' : 'primary'" @click="emit('confirm'); close()">{{ confirmText || 'Confirm' }}</GlassButton>
            </slot>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup>
import GlassButton from './GlassButton.vue'

const props = defineProps({
  modelValue: { type: Boolean, default: false },
  title: { type: String, default: '' },
  message: { type: String, default: '' },
  confirmText: { type: String, default: '' },
  cancelText: { type: String, default: '' },
  variant: { type: String, default: '' },
  width: { type: String, default: '480px' },
  persistent: { type: Boolean, default: false },
  closable: { type: Boolean, default: true },
})
const emit = defineEmits(['update:modelValue', 'confirm'])
function close() { if (props.closable) emit('update:modelValue', false) }
</script>

<style scoped>
.glass-dialog__overlay {
  position: fixed; inset: 0; z-index: var(--z-modal);
  display: flex; align-items: center; justify-content: center;
  padding: 24px;
  background: var(--overlay-scrim);
  backdrop-filter: blur(var(--blur-scrim));
  -webkit-backdrop-filter: blur(var(--blur-scrim));
}
.glass-dialog__panel {
  background: var(--bg-overlay);
  border: 1px solid var(--border-strong);
  border-radius: var(--radius-lg);
  width: 90%;
  box-shadow: 0 24px 48px rgba(0, 0, 0, 0.5), 0 0 0 1px rgba(255, 255, 255, 0.05) inset;
  display: flex;
  flex-direction: column;
  max-height: calc(100vh - 48px);
}
.glass-dialog__header {
  display: flex; align-items: center; justify-content: space-between;
  padding: 16px 20px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.glass-dialog__title {
  font-size: 1rem; font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}
.glass-dialog__close {
  background: none; border: none;
  color: var(--text-muted);
  font-size: 1.5rem; cursor: pointer;
  padding: 0; line-height: 1;
  width: 32px; height: 32px;
  display: flex; align-items: center; justify-content: center;
  border-radius: var(--radius-sm);
  transition: all var(--duration-fast);
}
.glass-dialog__close:hover {
  color: var(--text-primary);
  background: var(--hover-overlay);
}
.glass-dialog__body {
  padding: 20px;
  overflow-y: auto;
  flex: 1;
  background: var(--bg-raised);
}

/* Form layout within dialog body */
.glass-dialog__body :deep(.dialog-form) {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

/* Textarea styling within dialogs */
.glass-dialog__body :deep(.glass-textarea) {
  width: 100%;
  background: var(--input-bg);
  color: var(--text-primary);
  border: 1px solid var(--input-border);
  border-radius: var(--radius-sm);
  padding: 0.6rem 0.85rem;
  font-size: 0.85rem;
  font-family: inherit;
  resize: vertical;
  outline: none;
  transition: border-color var(--duration-fast), box-shadow var(--duration-fast);
}
.glass-dialog__body :deep(.glass-textarea:focus) {
  border-color: var(--input-focus-border);
  box-shadow: var(--input-focus-shadow);
}
.glass-dialog__body :deep(.glass-textarea::placeholder) {
  color: var(--text-muted);
}

/* Field labels within dialogs */
.glass-dialog__body :deep(.field-label) {
  display: block;
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 6px;
}

/* Hint text within dialogs */
.glass-dialog__body :deep(.dialog-hint) {
  font-size: 0.85rem;
  color: var(--text-secondary);
  margin: 0;
}

.glass-dialog__footer {
  display: flex; justify-content: flex-end; gap: 8px;
  padding: 16px 20px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.glass-dialog__message {
  color: var(--text-secondary);
  font-size: 0.9rem;
  margin: 0;
  line-height: 1.5;
}
/* Transitions */
.dialog-enter-active, .dialog-leave-active {
  transition: opacity var(--duration-normal) var(--ease-out);
}
.dialog-enter-active .glass-dialog__panel,
.dialog-leave-active .glass-dialog__panel {
  transition: transform var(--duration-normal) var(--ease-out);
}
.dialog-enter-from, .dialog-leave-to { opacity: 0; }
.dialog-enter-from .glass-dialog__panel {
  transform: scale(0.96) translateY(8px);
}
.dialog-leave-to .glass-dialog__panel {
  transform: scale(0.96);
}
</style>
