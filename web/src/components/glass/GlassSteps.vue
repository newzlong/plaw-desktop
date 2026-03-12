<template>
  <div class="glass-steps">
    <div
      v-for="(step, i) in steps"
      :key="i"
      class="glass-steps__item"
      :class="{
        'glass-steps__item--active': i === current,
        'glass-steps__item--done': i < current,
      }"
    >
      <div class="glass-steps__circle">
        <span v-if="i < current">&#10003;</span>
        <span v-else>{{ i + 1 }}</span>
      </div>
      <span class="glass-steps__label">{{ step }}</span>
      <div v-if="i < steps.length - 1" class="glass-steps__line" />
    </div>
  </div>
</template>

<script setup>
defineProps({
  steps: { type: Array, default: () => [] },
  current: { type: Number, default: 0 },
})
</script>

<style scoped>
.glass-steps { display: flex; align-items: center; }
.glass-steps__item {
  display: flex; align-items: center; gap: 0.5rem;
  flex: 1; position: relative;
}
.glass-steps__circle {
  width: 28px; height: 28px;
  border-radius: 50%;
  display: flex; align-items: center; justify-content: center;
  font-size: 0.75rem; font-weight: 600;
  background: var(--bg-surface);
  border: 2px solid var(--border-default);
  color: var(--text-muted);
  transition: all var(--duration-normal) var(--ease-out);
  flex-shrink: 0;
}
.glass-steps__item--active .glass-steps__circle {
  background: var(--plaw-primary);
  border-color: var(--plaw-primary);
  color: white;
  box-shadow: var(--shadow-glow);
}
.glass-steps__item--done .glass-steps__circle {
  background: var(--status-ok);
  border-color: var(--status-ok);
  color: white;
}
.glass-steps__label {
  font-size: 0.8rem;
  color: var(--text-muted);
  white-space: nowrap;
}
.glass-steps__item--active .glass-steps__label {
  color: var(--text-primary);
  font-weight: 600;
}
.glass-steps__item--done .glass-steps__label {
  color: var(--text-secondary);
}
.glass-steps__line {
  flex: 1; height: 2px;
  background: var(--border-default);
  margin: 0 0.5rem;
  border-radius: 1px;
}
.glass-steps__item--done .glass-steps__line {
  background: var(--status-ok);
}
</style>
