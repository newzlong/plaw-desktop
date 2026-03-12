import { ref, watch } from 'vue'
import { usePlawState } from './usePlawState'
import { getGatewayPort } from '../api/tauri'

/**
 * Global WebSocket connection (singleton).
 * Lives at App.vue level, shared by all components.
 * Reconnects automatically when Plaw is running.
 */

let ws = null
let reconnectTimer = null
let pingTimer = null

const wsConnected = ref(false)

// Typed message handlers: { type -> Set<callback> }
const handlers = {}

/** Register a handler for a specific message type */
function on(type, callback) {
  if (!handlers[type]) handlers[type] = new Set()
  handlers[type].add(callback)
  return () => handlers[type].delete(callback)
}

/** Register a catch-all handler */
const catchAllHandlers = new Set()
function onAny(callback) {
  catchAllHandlers.add(callback)
  return () => catchAllHandlers.delete(callback)
}

function dispatch(data) {
  const type = data.type || ''
  if (handlers[type]) {
    for (const cb of handlers[type]) cb(data)
  }
  for (const cb of catchAllHandlers) cb(data)
}

/** Send a message through the WebSocket */
function send(data) {
  if (ws && ws.readyState === WebSocket.OPEN) {
    ws.send(typeof data === 'string' ? data : JSON.stringify(data))
    return true
  }
  return false
}

function scheduleReconnect() {
  clearTimeout(reconnectTimer)
  reconnectTimer = setTimeout(connect, 3000)
}

function startPing() {
  clearInterval(pingTimer)
  // Send a lightweight ping every 25s to keep connection alive
  pingTimer = setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: 'ping' }))
    }
  }, 25000)
}

function stopPing() {
  clearInterval(pingTimer)
  pingTimer = null
}

async function connect() {
  if (ws && ws.readyState === WebSocket.OPEN) return

  if (ws) {
    ws.onclose = null
    ws.close()
    ws = null
  }

  const port = await getGatewayPort()
  if (!port) {
    wsConnected.value = false
    scheduleReconnect()
    return
  }

  try {
    ws = new WebSocket(`ws://127.0.0.1:${port}/ws/chat`)
  } catch {
    wsConnected.value = false
    scheduleReconnect()
    return
  }

  ws.onopen = () => {
    wsConnected.value = true
    startPing()
  }

  ws.onclose = () => {
    wsConnected.value = false
    stopPing()
    const { state } = usePlawState()
    if (['running', 'healthy'].includes(state.value)) {
      scheduleReconnect()
    }
  }

  ws.onerror = () => {
    wsConnected.value = false
  }

  ws.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data)
      dispatch(data)
    } catch { /* ignore malformed */ }
  }
}

function disconnect() {
  clearTimeout(reconnectTimer)
  stopPing()
  wsConnected.value = false
  if (ws) {
    ws.onclose = null
    ws.close()
    ws = null
  }
}

/** Initialize: call once from App.vue */
function init() {
  const { state } = usePlawState()

  watch(state, (newState) => {
    if (['stopped', 'crashed', 'stopping', 'restarting'].includes(newState)) {
      disconnect()
    } else if (['running', 'healthy'].includes(newState) && !wsConnected.value) {
      connect()
    }
  }, { immediate: true })

  // Reconnect when window regains visibility (comes back from tray)
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && !wsConnected.value) {
      const { state: s } = usePlawState()
      if (['running', 'healthy'].includes(s.value)) {
        connect()
      }
    }
  })
}

export function useWebSocket() {
  return { wsConnected, on, onAny, send, connect, disconnect, init }
}
