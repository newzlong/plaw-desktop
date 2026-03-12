<template>
  <div class="glass-card" :class="{ 'glass-card--hoverable': hoverable }">
    <div v-if="title" class="glass-card__header">
      <component v-if="icon" :is="icon" class="w-4 h-4" style="color: var(--lobster-primary)" />
      <h3 class="glass-card__title">{{ title }}</h3>
    </div>
    <slot />
  </div>
</template>

<script setup>
defineProps({
  title: { type: String, default: '' },
  icon: { type: [Object, Function], default: null },
  hoverable: { type: Boolean, default: true },
})
</script>

<style scoped>
.glass-card {
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  padding: 1.25rem;
  box-shadow: var(--shadow-card);
  transition: all var(--duration-normal) var(--ease-out);
}
.glass-card--hoverable:hover {
  border-color: var(--border-default);
  box-shadow: var(--shadow-card-hover);
  transform: translateY(-1px);
}
.glass-card__header {
  display: flex; align-items: center; gap: 8px;
  margin-bottom: 12px;
}
.glass-card__title {
  font-size: 0.8rem; font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
</style>
