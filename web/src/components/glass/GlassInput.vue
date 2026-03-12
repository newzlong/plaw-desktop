<template>
  <div class="glass-input-wrapper">
    <label v-if="label" class="glass-input__label">{{ label }}</label>
    <div class="glass-input__container">
      <input
        :type="showPassword ? 'text' : type"
        :value="modelValue"
        :placeholder="placeholder"
        :disabled="disabled"
        class="glass-input__field"
        @input="$emit('update:modelValue', $event.target.value)"
      />
      <button
        v-if="type === 'password'"
        class="glass-input__toggle"
        type="button"
        @click="showPassword = !showPassword"
      >
        {{ showPassword ? 'Hide' : 'Show' }}
      </button>
    </div>
    <p v-if="hint" class="glass-input__hint">{{ hint }}</p>
  </div>
</template>

<script setup>
import { ref } from 'vue'

defineProps({
  modelValue: { type: [String, Number], default: '' },
  label: { type: String, default: '' },
  type: { type: String, default: 'text' },
  placeholder: { type: String, default: '' },
  disabled: { type: Boolean, default: false },
  hint: { type: String, default: '' },
})
defineEmits(['update:modelValue'])

const showPassword = ref(false)
</script>

<style scoped>
.glass-input-wrapper { display: flex; flex-direction: column; gap: 6px; }
.glass-input__label {
  font-size: 0.8rem; font-weight: 600;
  color: var(--text-secondary);
}
.glass-input__container { position: relative; display: flex; }
.glass-input__field {
  flex: 1;
  background: var(--input-bg);
  border: 1px solid var(--input-border);
  border-radius: var(--radius-sm);
  padding: 0.6rem 0.85rem;
  color: var(--text-primary);
  font-size: 0.875rem;
  outline: none;
  transition: border-color var(--duration-fast), box-shadow var(--duration-fast);
  width: 100%;
}
.glass-input__field:focus {
  border-color: var(--input-focus-border);
  box-shadow: var(--input-focus-shadow);
}
.glass-input__field::placeholder { color: var(--text-muted); }
.glass-input__field:disabled { opacity: 0.5; cursor: not-allowed; }
.glass-input__toggle {
  position: absolute; right: 8px; top: 50%; transform: translateY(-50%);
  background: none; border: none;
  color: var(--text-muted);
  font-size: 0.7rem; cursor: pointer;
  padding: 0.25rem 0.5rem;
  transition: color var(--duration-fast);
}
.glass-input__toggle:hover { color: var(--plaw-primary); }
.glass-input__hint { font-size: 0.75rem; color: var(--text-muted); }
</style>
