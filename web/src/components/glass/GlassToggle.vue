<template>
  <label class="glass-toggle">
    <span v-if="label" class="glass-toggle__label">{{ label }}</span>
    <button
      type="button"
      role="switch"
      :aria-checked="modelValue"
      class="glass-toggle__track"
      :class="{ 'glass-toggle__track--on': modelValue }"
      @click="$emit('update:modelValue', !modelValue)"
    >
      <span class="glass-toggle__thumb" />
    </button>
  </label>
</template>

<script setup>
defineProps({
  modelValue: { type: Boolean, default: false },
  label: { type: String, default: '' },
})
defineEmits(['update:modelValue'])
</script>

<style scoped>
.glass-toggle { display: inline-flex; align-items: center; gap: 0.5rem; cursor: pointer; }
.glass-toggle__label {
  font-size: 0.85rem;
  color: var(--text-secondary);
}
.glass-toggle__track {
  position: relative;
  width: 40px; height: 22px;
  background: var(--border-default);
  border: none;
  border-radius: 11px;
  cursor: pointer;
  transition: background var(--duration-normal) var(--ease-out);
  padding: 0;
}
.glass-toggle__track:focus-visible {
  outline: none;
  box-shadow: var(--input-focus-shadow);
}
.glass-toggle__track--on {
  background: var(--lobster-primary);
}
.glass-toggle__thumb {
  position: absolute;
  top: 2px; left: 2px;
  width: 18px; height: 18px;
  background: white;
  border-radius: 50%;
  transition: transform var(--duration-normal) var(--ease-out);
  box-shadow: 0 1px 3px rgba(0,0,0,0.2);
}
.glass-toggle__track--on .glass-toggle__thumb {
  transform: translateX(18px);
}
</style>
