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

export async function startPlaw() {
  const inv = await getInvoke()
  return inv?.('start_plaw')
}

export async function stopPlaw() {
  const inv = await getInvoke()
  return inv?.('stop_plaw')
}

export async function restartPlaw() {
  const inv = await getInvoke()
  return inv?.('restart_plaw')
}

export async function getPlawStatus() {
  const inv = await getInvoke()
  return inv ? await inv('get_plaw_status') : false
}

/** Get full process state snapshot */
export async function getPlawState() {
  const inv = await getInvoke()
  return inv ? await inv('get_plaw_state') : { state: 'stopped', running: false, healthy: false, port: 0, started_at: null, crashed: false }
}

export async function getGatewayPort() {
  const inv = await getInvoke()
  return inv ? await inv('get_gateway_port') : 0
}

export async function getPlawStartedAt() {
  const inv = await getInvoke()
  return inv ? await inv('get_plaw_started_at') : null
}

export async function readConfig() {
  const inv = await getInvoke()
  return inv ? await inv('read_config') : {}
}

export async function writeConfig(config) {
  const inv = await getInvoke()
  return inv?.('write_config', { config })
}

export async function configExists() {
  const inv = await getInvoke()
  return inv ? await inv('config_exists') : false
}

export async function getRecentLogs(count = 200, level = null, keyword = null) {
  const inv = await getInvoke()
  return inv ? await inv('get_recent_logs', { count, level, keyword }) : []
}

export async function checkPlawHealth() {
  const inv = await getInvoke()
  return inv ? await inv('check_plaw_health') : false
}

export async function listLocalSkills() {
  const inv = await getInvoke()
  return inv ? await inv('list_local_skills') : []
}

export async function installSkill(pathOrUrl) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('install_skill', { pathOrUrl })
}

export async function uninstallSkill(name) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('uninstall_skill', { name })
}

export async function auditSkill(name) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('audit_skill', { name })
}

export async function auditAllUnaudited(force = false) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('audit_all_unaudited', { force })
}

export async function searchRegistrySkills(query = '') {
  const inv = await getInvoke()
  return inv ? await inv('search_registry_skills', { query }) : []
}

export async function syncSkillsRegistry() {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('sync_skills_registry')
}

export async function getMarketProxy() {
  const inv = await getInvoke()
  return inv ? await inv('get_market_proxy') : ''
}

export async function setMarketProxy(proxy) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('set_market_proxy', { proxy })
}

// Knowledge base
export async function listKnowledge() {
  const inv = await getInvoke()
  return inv ? await inv('list_knowledge') : []
}

export async function searchKnowledge(query) {
  const inv = await getInvoke()
  return inv ? await inv('search_knowledge', { query }) : []
}

export async function readKnowledgeEntry(id) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('read_knowledge_entry', { id })
}

export async function deleteKnowledgeEntry(id) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('delete_knowledge_entry', { id })
}

export async function saveKnowledgeEntry(title, tags, content, id = null) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('save_knowledge_entry', { title, tags, content, id })
}

export async function getKnowledgeStats() {
  const inv = await getInvoke()
  return inv ? await inv('get_knowledge_stats') : { total_entries: 0, total_tags: 0, top_tags: [], dir_path: '' }
}

export async function testProviderConnection(provider, apiKey, baseUrl, model, proxy) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('test_provider_connection', {
    provider,
    apiKey,
    baseUrl: baseUrl || null,
    model: model || null,
    proxy: proxy || null,
  })
}

// Embedding server
export async function startEmbedding() {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('start_embedding')
}

export async function stopEmbedding() {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('stop_embedding')
}

export async function getEmbeddingStatus() {
  const inv = await getInvoke()
  return inv ? await inv('get_embedding_status') : false
}

export async function isEmbeddingAvailable() {
  const inv = await getInvoke()
  return inv ? await inv('is_embedding_available') : false
}

// Chat sessions
export async function listSessions() {
  const inv = await getInvoke()
  return inv ? await inv('list_sessions') : []
}

export async function readSession(id) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('read_session', { id })
}

export async function saveSession(id, title, messages, contextUsed = 0, contextMax = 0) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('save_session', { id: id || null, title, messages, contextUsed: contextUsed || 0, contextMax: contextMax || 0 })
}

export async function deleteSession(id) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('delete_session', { id })
}

export async function appendSessionMessage(sessionId, role, content) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('append_session_message', { sessionId, role, content })
}

export async function sessionExists(id) {
  const inv = await getInvoke()
  return inv ? await inv('session_exists', { id }) : false
}

// Notifications
export async function addNotification(sessionId, source, jobId, jobName, content) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('add_notification', {
    sessionId: sessionId || null,
    source,
    jobId: jobId || null,
    jobName: jobName || null,
    content,
  })
}

export async function getSessionNotifications(sessionId) {
  const inv = await getInvoke()
  return inv ? await inv('get_session_notifications', { sessionId }) : []
}

export async function consumeNotifications(ids) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('consume_notifications', { ids })
}

export async function getAllUnconsumedNotifications() {
  const inv = await getInvoke()
  return inv ? await inv('get_all_unconsumed_notifications') : []
}

/** Send cancel to Plaw via Rust-side WS (failsafe for page unload) */
export async function cancelActiveChat() {
  const inv = await getInvoke()
  if (inv) await inv('cancel_active_chat')
}

/** Get uploads directory info: [totalBytes, fileCount] */
export async function getUploadsInfo() {
  const inv = await getInvoke()
  return inv ? await inv('get_uploads_info') : [0, 0]
}

/** Delete all files in uploads directory, returns [freedBytes, removedCount] */
export async function clearUploads() {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('clear_uploads')
}

/** Save an uploaded file to plaw-data/uploads/, returns the full path */
export async function saveUpload(name, data) {
  const inv = await getInvoke()
  if (!inv) throw new Error('Not in Tauri environment')
  return inv('save_upload', { name, data: Array.from(data) })
}
