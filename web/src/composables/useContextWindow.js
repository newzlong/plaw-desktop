import { ref, computed } from 'vue'

export function useContextWindow(isZh) {
  const contextUsed = ref(0)
  const contextMax = ref(0)
  const compacting = ref(false)
  let lastConfirmedTokens = 0

  function estimateTokens(msgs) {
    let chars = 0
    for (const m of msgs) chars += (m.content || '').length
    return Math.round(chars / 3) + 2000
  }

  const contextPercent = computed(() => {
    if (!contextMax.value || !contextUsed.value) return 0
    return Math.min(100, (contextUsed.value / contextMax.value) * 100)
  })

  const contextDotClass = computed(() => {
    const p = contextPercent.value
    if (p >= 85) return 'context-dot--critical'
    if (p >= 70) return 'context-dot--warning'
    if (p >= 50) return 'context-dot--moderate'
    return 'context-dot--ok'
  })

  const contextLabel = computed(() => {
    if (!contextMax.value) return '0K / 200K'
    const usedK = (contextUsed.value / 1000).toFixed(1)
    const maxK = Math.round(contextMax.value / 1000)
    return `${usedK}K / ${maxK}K`
  })

  const contextTooltip = computed(() => {
    const p = contextPercent.value.toFixed(2)
    return isZh.value
      ? `上下文用量 ${p}%（${contextLabel.value} tokens）`
      : `Context usage ${p}% (${contextLabel.value} tokens)`
  })

  function updateFromDone(data) {
    if (data.usage?.context_used) {
      contextUsed.value = data.usage.context_used
      lastConfirmedTokens = contextUsed.value
    }
    if (data.context_window) contextMax.value = data.context_window
  }

  function updateFromCompacted(data) {
    if (data.estimated_tokens) contextUsed.value = data.estimated_tokens
  }

  function initEstimate(msgs) {
    if (!contextUsed.value && !lastConfirmedTokens) {
      contextUsed.value = estimateTokens(msgs)
    }
    if (!contextMax.value) contextMax.value = 200000
  }

  function reset() {
    contextUsed.value = 0
    contextMax.value = 0
    lastConfirmedTokens = 0
  }

  function restoreFromSession(session) {
    if (session.context_used) contextUsed.value = session.context_used
    else contextUsed.value = estimateTokens(session.messages || [])
    if (session.context_max) contextMax.value = session.context_max
    else contextMax.value = 200000
  }

  return {
    contextUsed,
    contextMax,
    compacting,
    contextPercent,
    contextDotClass,
    contextLabel,
    contextTooltip,
    updateFromDone,
    updateFromCompacted,
    initEstimate,
    reset,
    restoreFromSession,
  }
}
