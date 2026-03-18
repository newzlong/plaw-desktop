<template>
  <div class="glass-skeleton" :style="skeletonStyle" />
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  width:   { type: String, default: '100%' },
  height:  { type: String, default: '1rem' },
  rounded: { type: Boolean, default: false },
  circle:  { type: Boolean, default: false },
})

const skeletonStyle = computed(() => ({
  width:        props.circle ? props.height : props.width,
  height:       props.height,
  borderRadius: props.circle ? '50%' : props.rounded ? '9999px' : 'var(--radius-sm)',
}))
</script>

<style scoped>
.glass-skeleton {
  background: linear-gradient(
    90deg,
    var(--bg-surface)       25%,
    var(--bg-surface-hover) 50%,
    var(--bg-surface)       75%
  );
  background-size: 200% 100%;
  animation: skeleton-shimmer 1.5s infinite;
  flex-shrink: 0;
}

@keyframes skeleton-shimmer {
  0%   { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
</style>
