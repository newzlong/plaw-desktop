<template>
  <div class="glass-select-wrapper">
    <label v-if="label" class="glass-select__label">{{ label }}</label>
    <div class="glass-select__container" ref="containerRef">
      <button
        class="glass-select__trigger"
        type="button"
        @click="open = !open"
        aria-haspopup="listbox"
        :aria-expanded="open"
      >
        <span :style="{ color: modelValue ? 'var(--text-primary)' : 'var(--text-muted)' }">
          {{ selectedLabel || placeholder }}
        </span>
        <span class="glass-select__arrow" :class="{ 'rotate-180': open }">&#9662;</span>
      </button>
      <Transition name="dropdown">
        <div v-if="open" class="glass-select__dropdown" role="listbox">
          <button
            v-for="opt in options"
            :key="opt.value"
            class="glass-select__option"
            :class="{ 'glass-select__option--active': opt.value === modelValue }"
            role="option"
            :aria-selected="opt.value === modelValue"
            @click="select(opt.value)"
          >
            {{ opt.label }}
          </button>
        </div>
      </Transition>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'

const props = defineProps({
  modelValue: { type: [String, Number], default: '' },
  label: { type: String, default: '' },
  placeholder: { type: String, default: 'Select...' },
  options: { type: Array, default: () => [] },
})
const emit = defineEmits(['update:modelValue'])

const open = ref(false)
const containerRef = ref(null)

const selectedLabel = computed(() => {
  const opt = props.options.find(o => o.value === props.modelValue)
  return opt?.label || ''
})

function select(val) {
  emit('update:modelValue', val)
  open.value = false
}

function onClickOutside(e) {
  if (containerRef.value && !containerRef.value.contains(e.target)) open.value = false
}
onMounted(() => document.addEventListener('click', onClickOutside))
onBeforeUnmount(() => document.removeEventListener('click', onClickOutside))
</script>

<style scoped>
.glass-select-wrapper { display: flex; flex-direction: column; gap: 6px; }
.glass-select__label {
  font-size: 0.8rem; font-weight: 600;
  color: var(--text-secondary);
}
.glass-select__container { position: relative; }
.glass-select__trigger {
  width: 100%;
  display: flex; align-items: center; justify-content: space-between;
  background: var(--input-bg);
  border: 1px solid var(--input-border);
  border-radius: var(--radius-sm);
  padding: 0.6rem 0.85rem;
  color: var(--text-primary);
  font-size: 0.875rem;
  cursor: pointer;
  transition: border-color var(--duration-fast), box-shadow var(--duration-fast);
}
.glass-select__trigger:hover { border-color: var(--border-strong); }
.glass-select__trigger:focus-visible {
  outline: none;
  box-shadow: var(--input-focus-shadow);
  border-color: var(--input-focus-border);
}
.glass-select__arrow {
  font-size: 0.7rem;
  color: var(--text-muted);
  transition: transform var(--duration-fast);
}
.glass-select__dropdown {
  position: absolute; top: calc(100% + 4px); left: 0; right: 0;
  background: var(--bg-overlay);
  backdrop-filter: blur(16px);
  -webkit-backdrop-filter: blur(16px);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  padding: 4px;
  z-index: 50;
  max-height: 200px; overflow-y: auto;
  box-shadow: 0 12px 32px rgba(0,0,0,0.2);
}
.glass-select__option {
  display: block; width: 100%;
  padding: 0.5rem 0.75rem;
  color: var(--text-primary);
  font-size: 0.85rem;
  background: none; border: none;
  border-radius: 6px;
  cursor: pointer; text-align: left;
  transition: background var(--duration-fast);
}
.glass-select__option:hover { background: var(--lobster-primary-soft); }
.glass-select__option--active {
  background: var(--lobster-primary-soft);
  color: var(--lobster-primary);
  font-weight: 600;
}
.dropdown-enter-active, .dropdown-leave-active {
  transition: all var(--duration-fast) var(--ease-out);
}
.dropdown-enter-from, .dropdown-leave-to {
  opacity: 0; transform: translateY(-4px);
}
</style>
