import { ref, computed } from 'vue'
import { getPlawState } from '../api/tauri'
import { listenStatus } from '../api/events'

/**
 * Global Plaw process state — singleton, survives component lifecycle.
 *
 * state values: 'stopped' | 'starting' | 'running' | 'healthy' |
 *               'stopping' | 'restarting' | 'crashed'
 */

// Module-level singleton state (shared across all components)
const state = ref('stopped')
const port = ref(0)
const startedAt = ref(null)
let initialized = false
let unlisten = null

function applySnapshot(s) {
  state.value = s.state
  port.value = s.port
  startedAt.value = s.started_at
}

async function init() {
  if (initialized) return
  initialized = true

  // Fetch current state from Rust
  try {
    const snapshot = await getPlawState()
    applySnapshot(snapshot)
  } catch { /* not in Tauri */ }

  // Subscribe to real-time status events
  try {
    unlisten = await listenStatus((snapshot) => {
      applySnapshot(snapshot)
    })
  } catch { /* not in Tauri */ }
}

export function usePlawState() {
  // Auto-init on first use
  init()

  const isRunning = computed(() =>
    ['running', 'healthy', 'starting'].includes(state.value)
  )
  const isHealthy = computed(() => state.value === 'healthy')
  const isBusy = computed(() =>
    ['starting', 'stopping', 'restarting'].includes(state.value)
  )
  const canStart = computed(() =>
    ['stopped', 'crashed'].includes(state.value)
  )
  const canStop = computed(() =>
    ['running', 'healthy'].includes(state.value)
  )

  return {
    state,
    port,
    startedAt,
    isRunning,
    isHealthy,
    isBusy,
    canStart,
    canStop,
  }
}
