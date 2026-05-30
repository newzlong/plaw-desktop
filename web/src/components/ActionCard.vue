<template>
  <div class="step-approval">
    <div class="step-approval__header">
      <span class="step-approval__icon" />
      <span class="step-approval__title">{{ isZh ? '需要授权' : 'Approval required' }}</span>
      <span class="step-approval__tool">{{ step.name }}</span>
    </div>
    <pre v-if="step.args" class="step-approval__args">{{ formattedArgs }}</pre>
    <!-- Shell-only: editable command prefix to remember (verbatim, user-editable). -->
    <label v-if="step.status === 'pending' && isShell" class="step-approval__prefix">
      <span class="step-approval__prefix-label">{{ isZh ? '记住前缀' : 'Remember prefix' }}</span>
      <input v-model="step.prefixInput" type="text" class="step-approval__prefix-input"
        :placeholder="isZh ? '例如：git status' : 'e.g. git status'" spellcheck="false" autocomplete="off" />
    </label>
    <div v-if="step.status === 'pending'" class="step-approval__actions">
      <GlassButton size="sm" variant="primary" @click="emit('decision', 'allow_once')">{{ isZh ? '允许一次' : 'Allow once' }}</GlassButton>
      <GlassButton size="sm" @click="emit('decision', 'allow_and_remember', step.prefixInput)">{{ isZh ? '允许并记住' : 'Allow & remember' }}</GlassButton>
      <GlassButton size="sm" variant="danger" @click="emit('decision', 'deny')">{{ isZh ? '拒绝' : 'Deny' }}</GlassButton>
    </div>
    <div v-else class="step-approval__resolved" :class="resolvedClass">
      <template v-if="step.decision === 'deny'">
        <span>{{ isZh ? '已拒绝' : 'Denied' }}</span>
      </template>
      <template v-else-if="step.decision === 'allow_once'">
        <span>{{ isZh ? '已允许一次' : 'Allowed once' }}</span>
      </template>
      <template v-else>
        <span>{{ isZh ? '已允许并记住' : 'Allowed and remembered' }}</span>
        <code v-if="step.rememberedPrefix" class="step-approval__remembered">{{ step.rememberedPrefix }}</code>
        <span v-else class="step-approval__remembered-note">{{ isZh ? '（整个工具）' : '(whole tool)' }}</span>
      </template>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import GlassButton from './glass/GlassButton.vue'

const props = defineProps({
  step: { type: Object, required: true },
  isZh: { type: Boolean, default: false },
})
const emit = defineEmits(['decision'])

const isShell = computed(() =>
  props.step?.name === 'shell' && typeof props.step?.args?.command === 'string'
)

const formattedArgs = computed(() => {
  const args = props.step?.args
  if (!args) return ''
  if (typeof args === 'string') return args
  return Object.entries(args)
    .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
    .join('\n')
})

// Distinct visual treatment per decision so allow-once vs remembered vs denied
// are immediately distinguishable at a glance.
const resolvedClass = computed(() => {
  if (props.step?.decision === 'deny') return 'step-approval__resolved--deny'
  if (props.step?.decision === 'allow_and_remember') return 'step-approval__resolved--remember'
  return 'step-approval__resolved--once'
})
</script>

<style scoped>
.step-approval {
  margin: 4px 0;
  padding: 10px 12px;
  border-radius: var(--radius-sm);
  background: var(--bg-raised);
  border: 1px solid var(--status-warn, var(--border-strong));
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.step-approval__header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.82rem;
}
.step-approval__icon {
  width: 8px;
  height: 8px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--status-warn, var(--plaw-accent));
}
.step-approval__title {
  font-weight: 600;
  color: var(--text-primary);
}
.step-approval__tool {
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.74rem;
  color: var(--text-muted);
}
.step-approval__args {
  margin: 0;
  padding: 6px 8px;
  background: var(--bg-base);
  border-radius: var(--radius-sm);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.74rem;
  line-height: 1.4;
  color: var(--text-secondary);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 160px;
  overflow-y: auto;
}
.step-approval__prefix {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.step-approval__prefix-label {
  font-size: 0.72rem;
  color: var(--text-muted);
}
.step-approval__prefix-input {
  width: 100%;
  box-sizing: border-box;
  padding: 5px 8px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-strong);
  background: var(--bg-base);
  color: var(--text-primary);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.74rem;
  outline: none;
}
.step-approval__prefix-input:focus {
  border-color: var(--status-warn, var(--plaw-accent));
}
.step-approval__actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.step-approval__resolved {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  font-size: 0.76rem;
  color: var(--text-muted);
  font-style: italic;
}
.step-approval__resolved--once {
  color: var(--status-ok, var(--text-secondary));
}
.step-approval__resolved--remember {
  color: var(--status-ok, var(--text-secondary));
  font-weight: 600;
  font-style: normal;
}
.step-approval__resolved--deny {
  color: var(--status-err, var(--text-muted));
  font-weight: 600;
}
.step-approval__remembered {
  padding: 1px 6px;
  background: var(--bg-base);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.72rem;
  font-weight: normal;
  font-style: normal;
  color: var(--text-secondary);
}
.step-approval__remembered-note {
  font-size: 0.72rem;
  font-weight: normal;
  color: var(--text-muted);
}
</style>
