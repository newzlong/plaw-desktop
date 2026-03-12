let _listen = null

async function getListen() {
  if (_listen) return _listen
  try {
    const m = await import('@tauri-apps/api/event')
    _listen = m.listen
  } catch {
    // Not in Tauri environment
  }
  return _listen
}

export async function listenCrash(callback) {
  // Legacy: kept for backwards compatibility, but plaw-status handles crashes too
  const listen = await getListen()
  if (!listen) return () => {}
  return listen('plaw-crashed', callback)
}

/**
 * Listen for real-time Plaw status updates.
 * Payload: { running, healthy, port, started_at, crashed }
 */
export async function listenStatus(callback) {
  const listen = await getListen()
  if (!listen) return () => {}
  return listen('plaw-status', (event) => {
    callback(event.payload)
  })
}

/**
 * Listen for Plaw activity events (from /api/events SSE stream).
 * Payload: { type, tool?, provider?, model?, duration_ms?, ... }
 * Event types: llm_request, tool_call_start, tool_call, agent_start, agent_end, error
 */
export async function listenActivity(callback) {
  const listen = await getListen()
  if (!listen) return () => {}
  return listen('plaw-activity', (event) => {
    callback(event.payload)
  })
}
