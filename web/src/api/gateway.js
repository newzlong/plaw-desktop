import { getGatewayPort } from './tauri'

let _invoke = null

async function getInvoke() {
  if (_invoke) return _invoke
  try {
    const m = await import('@tauri-apps/api/core')
    _invoke = m.invoke
  } catch {
    // Not in Tauri environment
  }
  return _invoke
}

async function get(path) {
  const inv = await getInvoke()
  if (inv) {
    try {
      return await inv('gateway_fetch', { path })
    } catch {
      return null
    }
  }
  const port = await getGatewayPort()
  if (!port) return null
  try {
    const res = await fetch(`http://127.0.0.1:${port}${path}`)
    if (!res.ok) return null
    return await res.json()
  } catch {
    return null
  }
}

async function post(path, body) {
  const inv = await getInvoke()
  if (inv) {
    try {
      return await inv('gateway_post', { path, body })
    } catch (e) {
      throw new Error(String(e))
    }
  }
  const port = await getGatewayPort()
  if (!port) throw new Error('Plaw not running')
  const res = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return await res.json()
}

async function patchReq(path, body) {
  const inv = await getInvoke()
  if (inv) {
    try {
      return await inv('gateway_patch', { path, body })
    } catch (e) {
      throw new Error(String(e))
    }
  }
  const port = await getGatewayPort()
  if (!port) throw new Error('Plaw not running')
  const res = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return await res.json()
}

async function del(path) {
  const inv = await getInvoke()
  if (inv) {
    try {
      return await inv('gateway_delete', { path })
    } catch (e) {
      throw new Error(String(e))
    }
  }
  const port = await getGatewayPort()
  if (!port) throw new Error('Plaw not running')
  const res = await fetch(`http://127.0.0.1:${port}${path}`, { method: 'DELETE' })
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  return await res.json()
}

export async function getSkills() { return get('/api/skills') }
export async function getChannels() { return get('/api/channels') }
export async function getAgents() { return get('/api/agents') }
export async function getCronJobs() { return get('/api/cron') }
export async function addCronJob(name, schedule, command) { return post('/api/cron', { name, schedule, command }) }
export async function deleteCronJob(id) { return del(`/api/cron/${id}`) }
export async function patchCronJob(id, body) { return patchReq(`/api/cron/${id}`, body) }
export async function getCronStatus() { return get('/api/cron') }
export async function getSessions() { return get('/api/sessions') }
export async function healthCheck() { return get('/health') }

export function resetPort() { /* no longer needed, kept for compatibility */ }
