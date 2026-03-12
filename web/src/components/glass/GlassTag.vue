<template>
  <div class="glass-tag-wrapper">
    <label v-if="label" class="glass-tag__label">{{ label }}</label>
    <div class="glass-tag__list">
      <span v-for="(tag, i) in modelValue" :key="i" class="glass-tag__item">
        {{ tag }}
        <button type="button" class="glass-tag__remove" @click="remove(i)">&times;</button>
      </span>
      <input
        v-model="input"
        class="glass-tag__input"
        :placeholder="modelValue.length ? '' : placeholder"
        @keydown.enter.prevent="add"
        @keydown.backspace="onBackspace"
      />
    </div>
  </div>
</template>

<script setup>
import { ref } from 'vue'

const props = defineProps({
  modelValue: { type: Array, default: () => [] },
  label: { type: String, default: '' },
  placeholder: { type: String, default: 'Type and press Enter' },
})
const emit = defineEmits(['update:modelValue'])

const input = ref('')

function add() {
  const val = input.value.trim()
  if (val && !props.modelValue.includes(val)) {
    emit('update:modelValue', [...props.modelValue, val])
    input.value = ''
  }
}

function remove(i) {
  const arr = [...props.modelValue]
  arr.splice(i, 1)
  emit('update:modelValue', arr)
}

function onBackspace() {
  if (!input.value && props.modelValue.length) {
    remove(props.modelValue.length - 1)
  }
}
</script>

<style scoped>
.glass-tag-wrapper { display: flex; flex-direction: column; gap: 6px; }
.glass-tag__label {
  font-size: 0.8rem; font-weight: 600;
  color: var(--text-secondary);
}
.glass-tag__list {
  display: flex; flex-wrap: wrap; gap: 6px;
  background: var(--input-bg);
  border: 1px solid var(--input-border);
  border-radius: var(--radius-sm);
  padding: 0.4rem 0.6rem;
  min-height: 38px;
  transition: border-color var(--duration-fast), box-shadow var(--duration-fast);
}
.glass-tag__list:focus-within {
  border-color: var(--input-focus-border);
  box-shadow: var(--input-focus-shadow);
}
.glass-tag__item {
  display: inline-flex; align-items: center; gap: 4px;
  background: var(--lobster-primary-soft);
  color: var(--lobster-primary);
  border-radius: 6px;
  padding: 0.15rem 0.5rem;
  font-size: 0.8rem; font-weight: 500;
}
.glass-tag__remove {
  background: none; border: none;
  color: var(--lobster-primary);
  cursor: pointer; font-size: 0.9rem;
  padding: 0 2px; line-height: 1;
  opacity: 0.6;
  transition: opacity var(--duration-fast);
}
.glass-tag__remove:hover { opacity: 1; }
.glass-tag__input {
  flex: 1; min-width: 60px;
  background: none; border: none; outline: none;
  color: var(--text-primary);
  font-size: 0.85rem;
}
.glass-tag__input::placeholder { color: var(--text-muted); }
</style>
