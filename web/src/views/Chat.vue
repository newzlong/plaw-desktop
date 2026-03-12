<template>
  <div class="chat-page">
    <!-- Session sidebar -->
    <div class="chat-sidebar">
      <button class="sidebar-new" :disabled="streaming" @click="newSession">
        <span>+</span> {{ t('chat.newChat') }}
      </button>
      <div class="sidebar-list">
        <div v-if="!sessions.length" class="sidebar-empty">
          {{ t('chat.noHistory') }}
        </div>
        <div
          v-for="s in sessions"
          :key="s.id"
          class="sidebar-item"
          :class="{ 'sidebar-item--active': s.id === currentSessionId, 'sidebar-item--disabled': streaming && s.id !== currentSessionId }"
          @click="!streaming && loadSession(s.id)"
        >
          <div class="sidebar-item__title">{{ s.title || t('chat.untitled') }}</div>
          <div class="sidebar-item__meta">{{ s.message_count }} {{ t('chat.placeholder').split(' ')[0] === '输入' ? '条' : 'msgs' }}</div>
          <button v-if="!streaming" class="sidebar-item__delete" @click.stop="removeSession(s.id)">×</button>
        </div>
      </div>

      <!-- Sidebar footer: plaw + settings + theme + language -->
      <div class="sidebar-footer">
        <button
          class="sidebar-footer__btn sidebar-footer__zc"
          :class="{
            'sidebar-footer__zc--running': canStop,
            'sidebar-footer__zc--stopped': canStart,
            'sidebar-footer__zc--busy': isBusy,
          }"
          :disabled="isBusy"
          :title="zcState"
          @click="togglePlaw"
        >
          <Loader2 v-if="isBusy" class="w-4 h-4 spin-icon" />
          <Square v-else-if="canStop" class="w-3.5 h-3.5" />
          <Play v-else class="w-4 h-4" />
        </button>
        <button class="sidebar-footer__btn" @click="showSettings = true" :title="t('settings.title')">
          <Settings class="w-4 h-4" />
        </button>
        <button class="sidebar-footer__btn" @click="toggleTheme" :title="appIsDark ? 'Light' : 'Dark'">
          <component :is="appIsDark ? Sun : Moon" class="w-4 h-4" />
        </button>
        <button class="sidebar-footer__btn sidebar-footer__lang" @click="appToggleLocale">
          {{ appIsZh ? 'EN' : '中' }}
        </button>
      </div>
    </div>

    <!-- Main chat area -->
    <div class="chat-main">
      <!-- Messages -->
      <div class="chat-messages" ref="messagesRef">
        <div v-if="!messages.length" class="chat-empty">
          <span class="chat-empty__emoji">🦞</span>
          <p>{{ t('chat.welcome') }}</p>
        </div>

        <div
          v-for="(msg, i) in messages"
          :key="i"
          class="chat-msg"
          :class="`chat-msg--${msg.role}`"
        >
          <div class="chat-msg__bubble">
            <!-- Rollback button for user messages -->
            <button
              v-if="msg.role === 'user' && !streaming && i !== messages.findIndex(m => m.role === 'user')"
              class="chat-msg__rollback"
              :title="t('chat.rollback')"
              @click="rollbackTo(i)"
            >
              <RotateCcw :size="13" />
            </button>
            <!-- Steps timeline (thinking + tool calls) -->
            <div v-if="msg.steps && msg.steps.length" class="chat-steps">
              <div v-for="(step, si) in msg.steps" :key="si" class="chat-step" :class="`chat-step--${step.type}`">
                <!-- Thinking step -->
                <div v-if="step.type === 'thinking'" class="step-thinking">
                  <span class="step-thinking__icon" />
                  <span class="step-thinking__text">{{ step.content }}</span>
                </div>
                <!-- Tool step -->
                <div v-else-if="step.type === 'tool'" class="step-tool-wrap">
                  <details class="step-tool">
                    <summary class="step-tool__header">
                      <span class="step-tool__dot" :class="step.status === 'running' ? 'step-tool__dot--running' : step.status === 'error' ? 'step-tool__dot--error' : 'step-tool__dot--done'" />
                      <span class="step-tool__name">{{ toolLabel(step.name) }}</span>
                      <span class="step-tool__raw-name">{{ step.name }}</span>
                    </summary>
                    <div class="step-tool__body">
                      <div v-if="step.args" class="step-tool__section">
                        <div class="step-tool__section-label">{{ t('chat.toolArgs') }}</div>
                        <pre class="step-tool__pre">{{ formatArgs(step.args) }}</pre>
                      </div>
                      <div v-if="step.output" class="step-tool__section">
                        <div class="step-tool__section-label">{{ t('chat.toolOutput') }}</div>
                        <pre class="step-tool__pre">{{ truncateOutput(step.output) }}</pre>
                      </div>
                    </div>
                  </details>
                  <div v-if="step.progress?.length" class="step-tool__progress">
                    <div v-for="(p, pi) in step.progress" :key="pi" class="step-tool__progress-line">{{ p }}</div>
                  </div>
                </div>
              </div>
            </div>
            <!-- User attached images -->
            <div v-if="msg.images && msg.images.length" class="chat-msg__images">
              <img v-for="(img, imgIdx) in msg.images" :key="imgIdx" :src="img" alt="user image" class="chat-msg__image" />
            </div>
            <!-- Text content -->
            <div v-if="msg.content" class="chat-msg__text">
              <span v-html="renderMarkdown(msg.content)" /><span v-if="streaming && i === messages.length - 1 && msg.role === 'assistant'" class="streaming-cursor" />
            </div>
          </div>
        </div>

        <div v-if="streaming && !currentAssistant.content && !currentAssistant.steps.length" class="chat-msg chat-msg--assistant">
          <div class="chat-msg__bubble">
            <div class="chat-typing">
              <span /><span /><span />
            </div>
          </div>
        </div>
      </div>

      <!-- Input -->
      <div class="chat-input-area">
        <div v-if="connStatus !== 'connected'" class="chat-disconnected" :class="`chat-disconnected--${connStatus}`">
          {{ connStatusText }}
        </div>
        <!-- File attachment previews -->
        <div v-if="attachedFiles.length" class="attached-files">
          <div v-for="(f, idx) in attachedFiles" :key="idx" class="attached-file" :class="{ 'attached-file--img': f.preview }">
            <img v-if="f.preview" :src="f.preview" alt="preview" />
            <div v-else class="attached-file__info">
              <span class="attached-file__name">{{ f.name }}</span>
              <span class="attached-file__size">{{ formatFileSize(f.size) }}</span>
            </div>
            <button class="attached-file__remove" @click="removeFile(idx)">&times;</button>
          </div>
        </div>
        <textarea
          ref="inputRef"
          v-model="inputText"
          :placeholder="t('chat.placeholder')"
          class="chat-input"
          :disabled="connStatus !== 'connected'"
          @keydown.enter.exact.prevent="sendMessage"
          @input="autoGrow"
          @paste="onPaste"
          @drop="onDrop"
          @dragover="onDragOver"
        />
        <input ref="fileInputRef" type="file" multiple hidden @change="onFileSelect" />
        <div class="chat-input-footer">
          <div class="chat-input-footer__left">
            <button class="attach-btn" :disabled="connStatus !== 'connected'" @click="fileInputRef?.click()" :title="isZh ? '添加文件' : 'Attach file'">
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M14 10l-4.5-4.5a2.12 2.12 0 0 0-3 3L11 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><path d="M11 13l1.5-1.5a3.18 3.18 0 0 0-4.5-4.5L2 13" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
            </button>
            <div class="context-indicator" :title="contextTooltip">
              <span class="context-dot" :class="contextDotClass" />
              <span class="context-text">{{ Math.round(contextPercent) }}% used</span>
            </div>
          </div>
          <button
            v-if="streaming && !inputText.trim()"
            class="chat-send chat-send--stop"
            @click="cancelMessage"
          >
            <span class="stop-icon" />
          </button>
          <button
            v-else
            class="chat-send"
            :class="{ 'chat-send--interrupt': streaming && inputText.trim() }"
            :disabled="(!inputText.trim() && !attachedFiles.length) || connStatus !== 'connected'"
            @click="sendMessage"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
              <path d="M7 12V2M7 2L2.5 6.5M7 2L11.5 6.5" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
        </div>
      </div>
    </div>

    <!-- Delete confirmation dialog -->
    <GlassDialog v-model="showDeleteDialog" :title="isZh ? '删除会话' : 'Delete Session'" width="340px">
      <p style="margin:0; font-size:0.88rem; color:var(--text-secondary)">
        {{ isZh ? '确定要删除这个会话吗？此操作不可撤销。' : 'Are you sure? This cannot be undone.' }}
      </p>
      <template #footer>
        <GlassButton @click="showDeleteDialog = false">{{ isZh ? '取消' : 'Cancel' }}</GlassButton>
        <GlassButton variant="danger" @click="confirmDelete">{{ isZh ? '删除' : 'Delete' }}</GlassButton>
      </template>
    </GlassDialog>

    <!-- Rollback confirmation dialog -->
    <GlassDialog v-model="showRollbackDialog" :title="t('chat.rollbackTitle')" width="340px">
      <p style="margin:0; font-size:0.88rem; color:var(--text-secondary)">
        {{ t('chat.rollbackConfirm') }}
      </p>
      <template #footer>
        <GlassButton @click="showRollbackDialog = false">{{ t('common.cancel') }}</GlassButton>
        <GlassButton variant="danger" @click="confirmRollback">{{ t('chat.rollback') }}</GlassButton>
      </template>
    </GlassDialog>
  </div>
</template>

<script setup>
import { ref, computed, nextTick, inject, onMounted, onUnmounted, onActivated, onDeactivated, watch } from 'vue'
import { Settings, Sun, Moon, Play, Square, Loader2, RotateCcw } from 'lucide-vue-next'

defineOptions({ name: 'ChatView' })
import { getGatewayPort } from '../api/tauri'
import { listSessions, readSession, saveSession, deleteSession, getSessionNotifications, consumeNotifications, cancelActiveChat, startPlaw, stopPlaw, auditAllUnaudited, saveUpload } from '../api/tauri'
import { usePlawState } from '../composables/usePlawState'
import { useI18n } from '../composables/useI18n'
import { listen } from '@tauri-apps/api/event'
import { useNotifications } from '../composables/useNotifications'
import { marked } from 'marked'
import GlassDialog from '../components/glass/GlassDialog.vue'
import GlassButton from '../components/glass/GlassButton.vue'

// Configure marked for safe, clean output
marked.setOptions({
  breaks: true,
  gfm: true,
})

const { addToast } = useNotifications()
let unlistenCronResult = null

const { t, isZh } = useI18n()
const { state: zcState, port: zcPort, isHealthy, canStart, canStop, isBusy } = usePlawState()

// Plaw start/stop toggle
async function togglePlaw() {
  if (isBusy.value) return
  try {
    if (canStop.value) await stopPlaw()
    else if (canStart.value) await startPlaw()
  } catch {}
}

// Injected from App.vue
const showSettings = inject('showSettings', ref(false))
const toggleTheme = inject('toggleTheme', () => {})
const appToggleLocale = inject('toggleLocale', () => {})
const appIsDark = inject('isDark', ref(false))
const appIsZh = inject('isZh', computed(() => false))

const messages = ref([])
const inputText = ref('')
const streaming = ref(false)
// WebSocket-level connection status
const wsConnected = ref(false)
const messagesRef = ref(null)
const inputRef = ref(null)
const fileInputRef = ref(null)

// File attachments: { name, type, size, file (File object), preview (data URI for images) }
const attachedFiles = ref([])
const MAX_FILE_SIZE = 50 * 1024 * 1024 // 50MB
const MAX_FILES = 10

const sessions = ref([])
const currentSessionId = ref(null)
let saveTimer = null

let ws = null
let currentAssistant = ref({ content: '', steps: [] })
let cancelled = false
const silentReconnect = ref(false)

// Delete confirmation
const confirmDeleteId = ref(null)
// Rollback confirmation
const confirmRollbackIndex = ref(null)
const showRollbackDialog = computed({
  get: () => confirmRollbackIndex.value !== null,
  set: (v) => { if (!v) confirmRollbackIndex.value = null },
})
const showDeleteDialog = computed({
  get: () => confirmDeleteId.value !== null,
  set: (v) => { if (!v) confirmDeleteId.value = null },
})

// Context window usage tracking
const contextUsed = ref(0)   // input_tokens from last done event
const contextMax = ref(0)    // max_context_tokens from server
let lastConfirmedTokens = 0  // last server-confirmed input_tokens (for streaming estimation)

/** Rough token estimate from message list (fallback when no server data) */
function estimateTokens(msgs) {
  let chars = 0
  for (const m of msgs) chars += (m.content || '').length
  // ~3 chars per token for mixed CJK/English; add overhead for system prompt (~2000 tokens)
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

// Typewriter buffer: content arrives in targetContent, displayLen advances per frame
let targetContent = ''
let displayLen = 0
let animFrame = null
let pendingFinalize = null  // holds done/error data until typewriter finishes
let pendingAutoContinue = false  // set by compacted event when pending tasks detected
let ignoreNextDone = false  // skip stale done/error after interrupt-and-send
let interruptTimer = null   // timeout fallback for stale done after cancel

function tickTypewriter() {
  if (displayLen >= targetContent.length) {
    animFrame = null
    if (pendingFinalize) doFinalize()
    return
  }
  const remaining = targetContent.length - displayLen
  // When all content has arrived (pendingFinalize), use visible typewriter speed
  // When streaming real-time chunks, catch up faster to avoid lag
  const speed = pendingFinalize
    ? Math.max(2, Math.min(Math.ceil(remaining * 0.04), 10))
    : Math.max(2, Math.ceil(remaining * 0.12))
  displayLen = Math.min(displayLen + speed, targetContent.length)
  currentAssistant.value.content = targetContent.slice(0, displayLen)
  updateLastAssistant()
  scrollToBottom()
  animFrame = requestAnimationFrame(tickTypewriter)
}

/** Flush typewriter instantly (for cancel/interrupt only) */
function flushTypewriter() {
  if (animFrame) { cancelAnimationFrame(animFrame); animFrame = null }
  pendingFinalize = null
  if (targetContent.length > displayLen) {
    displayLen = targetContent.length
    currentAssistant.value.content = targetContent
    updateLastAssistant()
  }
}

function resetTypewriter() {
  if (animFrame) { cancelAnimationFrame(animFrame); animFrame = null }
  pendingFinalize = null
  targetContent = ''
  displayLen = 0
}

/** Called when typewriter finishes playing out remaining content after done/error */
function doFinalize() {
  const fin = pendingFinalize
  pendingFinalize = null
  if (!fin) return

  streaming.value = false
  if (fin.type === 'done') {
    if (cancelled && !currentAssistant.value.content) {
      const last = messages.value[messages.value.length - 1]
      if (last && last.role === 'assistant') {
        last.content = '*[AI 回复被中断]*'
      }
    }
  } else if (fin.type === 'error') {
    if (cancelled) {
      if (!currentAssistant.value.content) {
        const last = messages.value[messages.value.length - 1]
        if (last && last.role === 'assistant') {
          last.content = '*[AI 回复被中断]*'
        }
      }
    } else {
      messages.value.push({ role: 'system', content: fin.message || 'Error' })
    }
  }
  cancelled = false
  resetTypewriter()
  currentAssistant.value = { content: '', steps: [] }
  scrollToBottom()
  scheduleSave()

  // Auto-continue pending tasks after compaction
  if (pendingAutoContinue && connStatus.value === 'connected') {
    pendingAutoContinue = false
    nextTick(() => {
      const continueMsg = isZh.value ? '请继续未完成的任务' : 'Please continue the pending tasks'
      inputText.value = continueMsg
      sendMessage()
    })
  }
}

// Derive connection status from global process state + WebSocket state
const connStatus = computed(() => {
  const s = zcState.value
  if (s === 'stopping' || s === 'restarting') return 'disconnecting'
  if (s === 'starting') return 'connecting'
  if (wsConnected.value || silentReconnect.value) return 'connected'
  if (['running', 'healthy'].includes(s)) return 'connecting'
  return 'disconnected'
})
const connStatusText = computed(() => t(`chat.conn_${connStatus.value}`))

// ---- Session management ----

async function refreshSessions() {
  sessions.value = await listSessions()
}

async function loadSession(id) {
  if (id === currentSessionId.value) return
  clearTimeout(saveTimer)
  await autoSave(true)

  try {
    const session = await readSession(id)
    currentSessionId.value = session.id
    sessionStorage.setItem('lobster-chat-session', session.id)
    messages.value = session.messages.map(m => ({
      role: m.role,
      content: m.content,
      steps: m.steps || [],
    }))
    // Restore context usage from saved session
    if (session.context_used) contextUsed.value = session.context_used
    else contextUsed.value = estimateTokens(messages.value)
    if (session.context_max) contextMax.value = session.context_max
    else contextMax.value = 200000  // default fallback
    scrollToBottom()
  } catch {
    // Session may have been deleted
    await refreshSessions()
  }
}

async function newSession() {
  clearTimeout(saveTimer)
  await autoSave(true)
  currentSessionId.value = null
  messages.value = []
  currentAssistant.value = { content: '', steps: [] }
  contextUsed.value = 0
  contextMax.value = 0
  lastConfirmedTokens = 0
  pendingAutoContinue = false
  pendingSendText = null
  ignoreNextDone = false
  clearTimeout(interruptTimer)
  sessionStorage.removeItem('lobster-chat-session')
}

async function removeSession(id) {
  confirmDeleteId.value = id
}

async function confirmDelete() {
  const id = confirmDeleteId.value
  confirmDeleteId.value = null
  if (!id) return
  // Cancel any pending debounced save to prevent it from re-creating the session
  if (currentSessionId.value === id) clearTimeout(saveTimer)
  await deleteSession(id)
  if (currentSessionId.value === id) {
    currentSessionId.value = null
    messages.value = []
    currentAssistant.value = { content: '', steps: [] }
    contextUsed.value = 0
    contextMax.value = 0
    lastConfirmedTokens = 0
    pendingAutoContinue = false
    pendingSendText = null
    ignoreNextDone = false
    clearTimeout(interruptTimer)
    sessionStorage.removeItem('lobster-chat-session')
  }
  await refreshSessions()
}

/** Generate a title from the first user message */
function generateTitle() {
  const firstUser = messages.value.find(m => m.role === 'user')
  if (!firstUser) return t('chat.untitled')
  const text = firstUser.content.trim()
  return text.length > 30 ? text.slice(0, 30) + '...' : text
}

/** Save current session to disk. If quiet=true, skip refreshing the session list. */
async function autoSave(quiet = false) {
  const saveable = messages.value.filter(m =>
    m.role === 'user' || (m.role === 'assistant' && m.content) || m.role === 'system'
  )
  if (saveable.length === 0) return

  const saveMsgs = saveable.map(m => {
    const obj = { role: m.role, content: m.content }
    if (m.steps && m.steps.length) obj.steps = m.steps
    return obj
  })
  const title = generateTitle()

  try {
    const saved = await saveSession(currentSessionId.value, title, saveMsgs, contextUsed.value, contextMax.value)
    currentSessionId.value = saved.id
    sessionStorage.setItem('lobster-chat-session', saved.id)
    if (!quiet) await refreshSessions()
  } catch {}
}

/** Debounced auto-save: triggers 2s after last message */
function scheduleSave() {
  clearTimeout(saveTimer)
  saveTimer = setTimeout(() => autoSave(), 2000)
}

// ---- Chat logic ----

function scrollToBottom() {
  nextTick(() => {
    if (messagesRef.value) {
      messagesRef.value.scrollTop = messagesRef.value.scrollHeight
    }
  })
}

const BASE_H = 42

function autoGrow() {
  const el = inputRef.value
  if (!el) return
  el.style.overflowY = 'hidden'
  el.style.height = BASE_H + 'px'
  const maxH = 200
  const sh = el.scrollHeight
  const h = Math.max(BASE_H, Math.min(sh, maxH))
  el.style.height = h + 'px'
  el.style.overflowY = sh > maxH ? 'auto' : 'hidden'
}

// ── File attachment helpers ──
function readFileAsDataURI(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result)
    reader.onerror = reject
    reader.readAsDataURL(file)
  })
}

function readFileAsArrayBuffer(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(new Uint8Array(reader.result))
    reader.onerror = reject
    reader.readAsArrayBuffer(file)
  })
}

async function addFiles(files) {
  for (const file of files) {
    if (attachedFiles.value.length >= MAX_FILES) break
    if (file.size > MAX_FILE_SIZE) continue
    const entry = { name: file.name, type: file.type, size: file.size, file }
    if (file.type.startsWith('image/')) {
      entry.preview = await readFileAsDataURI(file)
    }
    attachedFiles.value.push(entry)
  }
}

function removeFile(index) {
  attachedFiles.value.splice(index, 1)
}

function onFileSelect(e) {
  if (e.target.files) addFiles(Array.from(e.target.files))
  e.target.value = ''
}

function onPaste(e) {
  const items = e.clipboardData?.items
  if (!items) return
  const pasteFiles = []
  for (const item of items) {
    if (item.kind === 'file') {
      const file = item.getAsFile()
      if (file) pasteFiles.push(file)
    }
  }
  if (pasteFiles.length) {
    e.preventDefault()
    addFiles(pasteFiles)
  }
}

function onDrop(e) {
  e.preventDefault()
  if (e.dataTransfer?.files) addFiles(Array.from(e.dataTransfer.files))
}

function onDragOver(e) {
  e.preventDefault()
}

function formatFileSize(bytes) {
  if (bytes < 1024) return bytes + 'B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + 'KB'
  return (bytes / (1024 * 1024)).toFixed(1) + 'MB'
}

function renderMarkdown(text) {
  try {
    return marked.parse(text)
  } catch {
    // Fallback to escaped plain text
    return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/\n/g, '<br>')
  }
}

function formatArgs(args) {
  if (!args) return ''
  if (typeof args === 'string') return args
  return Object.entries(args)
    .map(([k, v]) => `${k}: ${typeof v === 'string' ? v : JSON.stringify(v)}`)
    .join('\n')
}

function truncateOutput(text, max = 300) {
  if (!text || text.length <= max) return text
  return text.slice(0, max) + '\n...'
}

function toolLabel(name) {
  const key = `chat.tools.${name}`
  const label = t(key)
  // If i18n returns the key itself, fall back to the raw name
  return label === key ? name : label
}

let reconnectTimer = null
let pendingSendText = null  // queued message to send after WS reconnects (interrupt-and-send)

function scheduleReconnect() {
  clearTimeout(reconnectTimer)
  reconnectTimer = setTimeout(connectWebSocket, 3000)
}

function closeWebSocket() {
  clearTimeout(reconnectTimer)
  clearTimeout(saveTimer)
  clearTimeout(interruptTimer)
  pendingSendText = null
  ignoreNextDone = false
  // If streaming, handle like stop button: save partial, mark interrupted if empty
  if (streaming.value) {
    flushTypewriter()
    updateLastAssistant()
    const last = messages.value[messages.value.length - 1]
    if (last && last.role === 'assistant' && !last.content) {
      last.content = '*[AI 回复被中断]*'
    }
    streaming.value = false
    cancelled = false
    ignoreNextDone = false
    resetTypewriter()
    currentAssistant.value = { content: '', steps: [] }
  }
  autoSave(true)
  wsConnected.value = false
  if (ws) {
    ws.onclose = null
    ws.close()
    ws = null
  }
}

async function connectWebSocket() {
  // Don't reconnect if already connected or still connecting
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return

  // Close stale socket if any
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

  const wsUrl = `ws://127.0.0.1:${port}/ws/chat`

  try {
    ws = new WebSocket(wsUrl)
  } catch {
    wsConnected.value = false
    scheduleReconnect()
    return
  }

  ws.onopen = () => {
    wsConnected.value = true
    silentReconnect.value = false
    // Flush queued message from interrupt-and-send (user msg already in messages[])
    if (pendingSendText) {
      const text = pendingSendText
      pendingSendText = null
      doSendAfterInterrupt(text)
    }
  }

  ws.onclose = () => {
    wsConnected.value = false
    // If typewriter still playing with pendingFinalize, let it finish
    if (!pendingFinalize) {
      streaming.value = false
      cancelled = false
    }
    // Only auto-reconnect if Plaw is supposed to be running
    if (['running', 'healthy'].includes(zcState.value)) {
      scheduleReconnect()
    } else {
      silentReconnect.value = false
    }
  }

  ws.onerror = () => {
    wsConnected.value = false
  }

  ws.onmessage = (event) => {
    try {
      const data = JSON.parse(event.data)
      handleWsMessage(data)
    } catch {}
  }
}

function handleWsMessage(data) {
  const type = data.type || ''

  // Handle stale done/error from a cancelled request (user follow-up interrupt).
  // The cancelled done arrives from the interrupted agent loop; the new loop is
  // already starting server-side. Just consume it and keep streaming.
  if (ignoreNextDone && (type === 'done' || type === 'error')) {
    ignoreNextDone = false
    clearTimeout(interruptTimer)
    return
  }

  // Ignore messages when not streaming (e.g. stale responses after cancel)
  if (!streaming.value) return

  if (type === 'thinking') {
    const steps = currentAssistant.value.steps
    const last = steps[steps.length - 1]
    // Consecutive thinking messages update the same step
    if (last && last.type === 'thinking') {
      last.content = data.content || ''
    } else {
      steps.push({ type: 'thinking', content: data.content || '' })
    }
    updateLastAssistant()
    scrollToBottom()
  } else if (type === 'chunk') {
    targetContent += data.content || ''
    // Start typewriter animation if not already running
    if (!animFrame) {
      animFrame = requestAnimationFrame(tickTypewriter)
    }
  } else if (type === 'tool_call' || type === 'tool_call_start') {
    currentAssistant.value.steps.push({
      type: 'tool',
      name: data.name || 'unknown',
      args: data.args || null,
      status: 'running',
      output: '',
    })
    updateLastAssistant()
    scrollToBottom()
  } else if (type === 'tool_result') {
    const steps = currentAssistant.value.steps
    // Find the last running tool with matching name
    const toolStep = [...steps].reverse().find(
      s => s.type === 'tool' && s.name === (data.name || '') && s.status === 'running'
    )
    if (toolStep) {
      const output = data.output || ''
      toolStep.status = output.startsWith('failed') ? 'error' : 'done'
      toolStep.output = output
    }
    updateLastAssistant()
    scrollToBottom()
  } else if (type === 'tool_progress') {
    const steps = currentAssistant.value.steps
    const toolStep = [...steps].reverse().find(
      s => s.type === 'tool' && s.name === (data.name || '') && s.status === 'running'
    )
    if (toolStep) {
      if (!toolStep.progress) toolStep.progress = []
      toolStep.progress.push(data.message || '')
    }
    updateLastAssistant()
    scrollToBottom()
  } else if (type === 'done') {
    // Server sends context_used = full history size (next request's expected input).
    if (data.usage?.context_used) {
      contextUsed.value = data.usage.context_used
      lastConfirmedTokens = contextUsed.value
    }
    if (data.context_window) contextMax.value = data.context_window
    // If full_response arrived but no chunks were streamed, feed it to typewriter
    if (data.full_response && !targetContent) {
      targetContent = data.full_response
    }
    // Defer cleanup: let typewriter play out remaining content, then finalize
    pendingFinalize = { type: 'done' }
    if (displayLen >= targetContent.length) {
      doFinalize()
    } else if (!animFrame) {
      animFrame = requestAnimationFrame(tickTypewriter)
    }
  } else if (type === 'error') {
    pendingFinalize = { type: 'error', message: data.message }
    if (displayLen >= targetContent.length) {
      doFinalize()
    } else if (!animFrame) {
      animFrame = requestAnimationFrame(tickTypewriter)
    }
  } else if (type === 'compacted') {
    // Context was auto-compacted — update progress bar with post-compaction estimate
    if (data.estimated_tokens) contextUsed.value = data.estimated_tokens
    const hasPending = data.has_pending_tasks
    const notice = isZh.value
      ? `上下文已自动压缩（剩余 ${data.remaining_messages} 条消息，约 ${Math.round((data.estimated_tokens || 0) / 1000)}K tokens）${hasPending ? '\n检测到未完成的任务，正在自动继续...' : ''}`
      : `Context auto-compacted (${data.remaining_messages} messages remaining, ~${Math.round((data.estimated_tokens || 0) / 1000)}K tokens)${hasPending ? '\nPending tasks detected, auto-continuing...' : ''}`
    messages.value.push({ role: 'system', content: notice })
    scrollToBottom()
    // Auto-continue if there are pending tasks (wait for typewriter/streaming to finish)
    if (hasPending) {
      pendingAutoContinue = true
    }
  } else if (type === 'skills_reloaded') {
    const names = (data.new_skills || []).map(s => s.name).join(', ')
    const count = (data.new_skills || []).length
    const notice = isZh.value
      ? `已检测到 ${count} 个新技能并自动加载：${names}（共 ${data.total_skills} 个技能）`
      : `${count} new skill(s) detected and loaded: ${names} (${data.total_skills} total)`
    messages.value.push({ role: 'system', content: notice })
    scrollToBottom()
    // Auto-audit newly installed skills
    if (count > 0) {
      auditAllUnaudited().catch(() => {})
    }
  }
}

function updateLastAssistant() {
  const last = messages.value[messages.value.length - 1]
  if (last && last.role === 'assistant') {
    last.content = currentAssistant.value.content
    last.steps = currentAssistant.value.steps.map(s => ({ ...s }))
  }
}

function rollbackTo(index) {
  if (streaming.value) return
  confirmRollbackIndex.value = index
}

async function confirmRollback() {
  const index = confirmRollbackIndex.value
  confirmRollbackIndex.value = null
  if (index == null) return
  const text = messages.value[index]?.content || ''
  const removed = messages.value.slice(index)
  const hadTools = removed.some(m => m.steps?.some(s => s.type === 'tool'))
  messages.value.splice(index)
  inputText.value = text
  if (hadTools) {
    addToast(t('chat.rollbackWarn'), 'warning')
  }
  // If all messages were rolled back, delete the session file
  const hasContent = messages.value.some(m =>
    m.role === 'user' || (m.role === 'assistant' && m.content)
  )
  if (!hasContent && currentSessionId.value) {
    await deleteSession(currentSessionId.value)
    currentSessionId.value = null
    sessionStorage.removeItem('lobster-chat-session')
    await refreshSessions()
  } else {
    await autoSave()
  }
  nextTick(() => {
    if (inputRef.value) {
      inputRef.value.focus()
      inputRef.value.style.height = 'auto'
      inputRef.value.style.height = Math.min(inputRef.value.scrollHeight, 200) + 'px'
    }
  })
}

/** Save file entries via Tauri, return array of { name, path, isImage } */
async function saveFilesToDisk(entries) {
  const results = []
  for (const entry of entries) {
    try {
      const bytes = await readFileAsArrayBuffer(entry.file)
      const savedPath = await saveUpload(entry.name, bytes)
      results.push({ name: entry.name, path: savedPath, isImage: entry.type.startsWith('image/') })
    } catch (e) {
      console.warn('Failed to save upload:', entry.name, e)
    }
  }
  return results
}

function buildContentWithFiles(text, savedFiles) {
  if (!savedFiles.length) return text
  let content = text
  for (const f of savedFiles) {
    if (f.isImage) {
      content += `\n[IMAGE:${f.path}]`
    } else {
      content += `\n[用户附件: ${f.name}, 路径: ${f.path}]`
    }
  }
  return content
}

function sendMessage() {
  const text = inputText.value.trim()
  if ((!text && !attachedFiles.value.length) || connStatus.value !== 'connected') return

  if (streaming.value) {
    // User sent a follow-up message while AI is working.
    // Send the message directly to Plaw — the server will automatically
    // cancel the current agent loop and process the new message.
    // 1. Finalize current assistant response locally
    flushTypewriter()
    updateLastAssistant()
    const last = messages.value[messages.value.length - 1]
    if (last && last.role === 'assistant' && !last.content) {
      last.content = '*[AI 回复被中断]*'
    }
    cancelled = false
    pendingAutoContinue = false
    resetTypewriter()
    currentAssistant.value = { content: '', steps: [] }
    // 2. Push user message + new empty assistant for the follow-up
    const interruptFiles = [...attachedFiles.value]
    const interruptPreviews = interruptFiles.filter(f => f.preview).map(f => f.preview)
    const interruptDisplay = text + (interruptFiles.length ? `\n📎 ${interruptFiles.map(f => f.name).join(', ')}` : '')
    messages.value.push({ role: 'user', content: interruptDisplay, images: interruptPreviews })
    messages.value.push({ role: 'assistant', content: '', steps: [] })
    attachedFiles.value = []
    inputText.value = ''
    scheduleSave()
    scrollToBottom()
    // 3. Ignore the cancelled done event from the interrupted loop
    ignoreNextDone = true
    clearTimeout(interruptTimer)
    interruptTimer = setTimeout(() => { ignoreNextDone = false }, 5000)
    // 4. Send the message to Plaw — server handles cancel + immediate reprocessing
    saveFilesToDisk(interruptFiles).then(savedFiles => {
      const fullContent = buildContentWithFiles(text, savedFiles)
      if (ws && ws.readyState === WebSocket.OPEN) {
        actuallySendWs(fullContent)
      } else {
        pendingSendText = fullContent
      }
    }).catch(() => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        actuallySendWs(text)
      } else {
        pendingSendText = text
      }
    })
    nextTick(() => {
      if (inputRef.value) {
        inputRef.value.style.height = BASE_H + 'px'
        inputRef.value.style.overflowY = 'hidden'
      }
    })
    return
  }

  doSendMessage(text)
}

/** Core send logic — assumes not currently streaming */
async function doSendMessage(text) {
  cancelled = false
  resetTypewriter()

  // Save files and build content
  const filesToSave = [...attachedFiles.value]
  const fileNames = filesToSave.map(f => f.name)
  const filePreviews = filesToSave.filter(f => f.preview).map(f => f.preview)
  // Show user message immediately with file names
  const displayContent = text + (fileNames.length ? `\n📎 ${fileNames.join(', ')}` : '')
  messages.value.push({ role: 'user', content: displayContent, images: filePreviews })
  messages.value.push({ role: 'assistant', content: '', steps: [] })
  currentAssistant.value = { content: '', steps: [] }
  attachedFiles.value = []

  // Context bar: keep last server-confirmed value (updated on 'done' events).
  // Only use rough estimate for the very first message when no server data exists.
  if (!contextUsed.value && !lastConfirmedTokens) {
    contextUsed.value = estimateTokens(messages.value)
  }
  if (!contextMax.value) contextMax.value = 200000

  inputText.value = ''
  streaming.value = true
  scrollToBottom()
  scheduleSave()

  nextTick(() => {
    if (inputRef.value) {
      inputRef.value.style.height = BASE_H + 'px'
      inputRef.value.style.overflowY = 'hidden'
    }
  })

  // Save files to disk and build the full content with file paths
  const savedFiles = filesToSave.length ? await saveFilesToDisk(filesToSave) : []
  const fullContent = buildContentWithFiles(text, savedFiles)

  // If WS is not open (e.g. Plaw closed it after cancel), queue for after reconnect
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    pendingSendText = fullContent
    return
  }

  actuallySendWs(fullContent)
}

/** Send follow-up after interrupt — user message was already pushed by sendMessage */
function doSendAfterInterrupt(text) {
  cancelled = false
  resetTypewriter()
  messages.value.push({ role: 'assistant', content: '', steps: [] })
  currentAssistant.value = { content: '', steps: [] }
  streaming.value = true
  scrollToBottom()
  scheduleSave()

  if (!ws || ws.readyState !== WebSocket.OPEN) {
    pendingSendText = text
    return
  }
  actuallySendWs(text)
}

/** Send the WS message payload */
function actuallySendWs(text) {
  // Build history for Plaw context restore
  const history = messages.value.slice(0, -1)
    .filter(m => m.role === 'user' || (m.role === 'assistant' && m.content))
    .map(m => ({ role: m.role, content: m.content }))

  try {
    ws.send(JSON.stringify({
      type: 'message',
      content: text,
      history: history.slice(0, -1),
      session_id: currentSessionId.value || undefined,
    }))
  } catch {
    // WS send failed — queue for reconnect
    pendingSendText = text
  }
}

/** User clicked stop button — just send cancel, let WS drain remaining content */
function cancelMessage() {
  if (!streaming.value) return
  cancelled = true
  silentReconnect.value = true
  if (ws && ws.readyState === WebSocket.OPEN) {
    try { ws.send(JSON.stringify({ type: 'cancel' })) } catch {}
  }
}

// React to global process state changes
watch(zcState, (newState) => {
  if (['stopped', 'crashed', 'stopping', 'restarting'].includes(newState)) {
    closeWebSocket()
  } else if (['running', 'healthy'].includes(newState) && !wsConnected.value) {
    connectWebSocket()
  }
})

onMounted(async () => {
  await refreshSessions()
  // Restore last active session
  const lastId = sessionStorage.getItem('lobster-chat-session')
  if (lastId) {
    try {
      const session = await readSession(lastId)
      currentSessionId.value = session.id
      messages.value = session.messages.map(m => ({
        role: m.role,
        content: m.content,
        steps: m.steps || [],
      }))
      // Restore context usage from saved session
      if (session.context_used) contextUsed.value = session.context_used
      else contextUsed.value = estimateTokens(messages.value)
      if (session.context_max) contextMax.value = session.context_max
      else contextMax.value = 200000
    } catch {
      // Session may have been deleted, try the most recent
    }
  }
  if (!currentSessionId.value && sessions.value.length > 0) {
    await loadSession(sessions.value[0].id)
  }
  // Inject pending notifications for this session
  if (currentSessionId.value) {
    try {
      const pending = await getSessionNotifications(currentSessionId.value)
      if (pending.length > 0) {
        for (const n of pending) {
          messages.value.push({ role: 'system', content: n.content, steps: [] })
        }
        await consumeNotifications(pending.map(n => n.id))
        scrollToBottom()
        scheduleSave()
      }
    } catch {}
  }
  // Connect if Plaw is already running
  if (['running', 'healthy'].includes(zcState.value)) {
    connectWebSocket()
  }

  // Register beforeunload guard: cancel AI + save if page refreshes/closes
  window.addEventListener('beforeunload', handleBeforeUnload)

  // Listen for cron-result Tauri events (from Rust SSE watcher)
  // If user is viewing the target session, insert inline message in real-time
  unlistenCronResult = await listen('cron-result', (event) => {
    const data = event.payload
    if (!data || data.type !== 'cron_result') return
    const targetSession = data.lobster_session || null
    if (targetSession && targetSession === currentSessionId.value) {
      const name = data.job_name || 'cron'
      const ok = data.status === 'ok'
      const output = data.output || ''
      const preview = output.length > 200 ? output.slice(0, 200) + '...' : output
      const content = `${ok ? '\u2705' : '\u274C'} \u5B9A\u65F6\u4EFB\u52A1 "${name}" \u6267\u884C\u5B8C\u6210\n${preview}`
      messages.value.push({ role: 'system', content, steps: [] })
      scrollToBottom()
    }
  })
})

/**
 * Emergency interrupt: cancel AI, mark message as interrupted, save to disk.
 * Called from beforeunload / onDeactivated / onUnmounted as last-resort protection.
 */
function interruptAndSave() {
  if (!streaming.value) return
  // 1. Send cancel via frontend WS (best effort)
  if (ws && ws.readyState === WebSocket.OPEN) {
    try { ws.send(JSON.stringify({ type: 'cancel' })) } catch {}
  }
  // 2. Ask Rust to send cancel via its own WS connection (failsafe)
  cancelActiveChat().catch(() => {})
  // 3. Save partial response; only mark interrupted if no content yet
  flushTypewriter()
  updateLastAssistant()
  const last = messages.value[messages.value.length - 1]
  if (last && last.role === 'assistant' && !last.content) {
    last.content = '*[AI 回复被中断]*'
  }
  streaming.value = false
  cancelled = false
  resetTypewriter()
  currentAssistant.value = { content: '', steps: [] }
  autoSave(true)
}

function handleBeforeUnload() {
  interruptAndSave()
}

onActivated(async () => {
  refreshSessions()
  scrollToBottom()
  // Reconnect if needed (state is already correct from global)
  if (['running', 'healthy'].includes(zcState.value) && !wsConnected.value) {
    connectWebSocket()
  }
  // Load pending notifications written by Rust SSE watcher while page was away
  if (currentSessionId.value) {
    try {
      const pending = await getSessionNotifications(currentSessionId.value)
      if (pending.length > 0) {
        for (const n of pending) {
          // Avoid duplicates: skip if last message already matches
          const last = messages.value[messages.value.length - 1]
          if (last && last.content === n.content) continue
          messages.value.push({ role: 'system', content: n.content, steps: [] })
        }
        await consumeNotifications(pending.map(n => n.id))
        scrollToBottom()
      }
    } catch {}
  }
})

onDeactivated(() => {
  // Chat keeps running in background (keep-alive preserves component instance).
  // WS connection + streaming state remain intact — no interrupt needed.
})

onUnmounted(() => {
  window.removeEventListener('beforeunload', handleBeforeUnload)
  interruptAndSave()
  resetTypewriter()
  clearTimeout(saveTimer)
  clearTimeout(reconnectTimer)
  clearTimeout(interruptTimer)
  if (ws) {
    ws.onclose = null
    ws.close()
  }
  if (unlistenCronResult) unlistenCronResult()
})
</script>

<style scoped>
.chat-page {
  display: flex;
  height: 100vh;
  width: 100%;
}

/* ---- Sidebar ---- */
.chat-sidebar {
  width: 220px;
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  margin: 8px 0 8px 8px;
  background: var(--sidebar-bg);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  border: 1px solid var(--sidebar-border);
  border-radius: var(--radius-lg);
  overflow: hidden;
}

.sidebar-new {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 8px 8px 0;
  padding: 10px 14px;
  background: var(--lobster-primary-soft);
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  color: var(--lobster-primary);
  font-size: 0.84rem;
  font-weight: 600;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-new:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.sidebar-new:not(:disabled):hover {
  background: var(--lobster-primary);
  color: white;
  border-color: var(--lobster-primary);
}
.sidebar-new span {
  font-size: 1.1rem;
}

.sidebar-list {
  flex: 1;
  overflow-y: auto;
  padding: 6px 8px;
}

.sidebar-empty {
  padding: 20px 16px;
  color: var(--text-muted);
  font-size: 0.8rem;
  text-align: center;
}

.sidebar-item {
  position: relative;
  padding: 10px 12px;
  padding-left: 9px;
  margin-bottom: 2px;
  cursor: pointer;
  border-radius: var(--radius-sm);
  border-left: 3px solid transparent;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-item:not(.sidebar-item--disabled):hover {
  background: var(--bg-surface-hover);
}
.sidebar-item--disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
.sidebar-item--active {
  background: var(--lobster-primary-soft);
  border-left-color: var(--lobster-primary);
}

.sidebar-item__title {
  font-size: 0.82rem;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  padding-right: 20px;
  transition: color var(--duration-fast) var(--ease-out);
}
.sidebar-item--active .sidebar-item__title {
  color: var(--lobster-primary);
  font-weight: 600;
}
.sidebar-item__meta {
  font-size: 0.72rem;
  color: var(--text-muted);
  margin-top: 3px;
}
.sidebar-item__delete {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  width: 22px;
  height: 22px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-sm);
  border: none;
  background: none;
  color: var(--text-muted);
  font-size: 0.9rem;
  cursor: pointer;
  opacity: 0;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-item:hover .sidebar-item__delete {
  opacity: 1;
}
.sidebar-item__delete:hover {
  color: var(--status-err);
  background: var(--lobster-primary-soft);
}

/* ---- Sidebar footer ---- */
.sidebar-footer {
  display: flex;
  justify-content: center;
  gap: 6px;
  padding: 8px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.sidebar-footer__btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  background: var(--input-bg);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  color: var(--text-muted);
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.sidebar-footer__btn:hover {
  background: var(--lobster-primary-soft);
  color: var(--lobster-primary);
  border-color: var(--lobster-primary-soft);
}
.sidebar-footer__lang {
  font-size: 0.72rem;
  font-weight: 700;
  width: auto;
  padding: 0 10px;
}

/* Plaw toggle button */
.sidebar-footer__zc--running {
  color: var(--status-ok) !important;
  border-color: rgba(34, 197, 94, 0.3);
}
.sidebar-footer__zc--running:hover {
  background: rgba(239, 68, 68, 0.12) !important;
  color: var(--status-err) !important;
  border-color: rgba(239, 68, 68, 0.3) !important;
}
.sidebar-footer__zc--stopped {
  color: var(--text-muted) !important;
}
.sidebar-footer__zc--stopped:hover {
  background: rgba(34, 197, 94, 0.12) !important;
  color: var(--status-ok) !important;
  border-color: rgba(34, 197, 94, 0.3) !important;
}
.sidebar-footer__zc--busy {
  opacity: 0.6;
  cursor: not-allowed !important;
}
.spin-icon {
  animation: spin-zc 1s linear infinite;
}
@keyframes spin-zc {
  to { transform: rotate(360deg); }
}

/* ---- Main chat ---- */
.chat-main {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-width: 0;
}

.chat-messages {
  flex: 1;
  overflow-y: auto;
  padding: 24px;
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.chat-empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: var(--text-muted);
  font-size: 0.9rem;
  gap: 12px;
}
.chat-empty__emoji {
  font-size: 3rem;
}

/* Messages */
.chat-msg {
  display: flex;
  max-width: 80%;
}
.chat-msg--user {
  align-self: flex-end;
}
.chat-msg--assistant {
  align-self: flex-start;
}
.chat-msg--system {
  align-self: center;
  max-width: 90%;
}

.chat-msg__bubble {
  position: relative;
  padding: 12px 16px;
  border-radius: var(--radius-md);
  font-size: 0.88rem;
  line-height: 1.55;
  word-break: break-word;
}
.chat-msg--user .chat-msg__bubble {
  background: var(--lobster-primary);
  color: white;
  border-bottom-right-radius: 4px;
}
.chat-msg__rollback {
  position: absolute;
  left: -28px;
  top: 50%;
  transform: translateY(-50%);
  background: none;
  border: none;
  cursor: pointer;
  color: var(--text-muted);
  padding: 4px;
  border-radius: var(--radius-sm);
  opacity: 0;
  transition: opacity var(--duration-fast), color var(--duration-fast);
  display: flex;
  align-items: center;
}
.chat-msg--user:hover .chat-msg__rollback {
  opacity: 0.6;
}
.chat-msg__rollback:hover {
  opacity: 1 !important;
  color: var(--text-primary);
}
.chat-msg--assistant .chat-msg__bubble {
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  color: var(--text-primary);
  border-bottom-left-radius: 4px;
}
.chat-msg--system .chat-msg__bubble {
  background: var(--lobster-accent-soft);
  border: 1px solid var(--border-default);
  color: var(--text-secondary);
  font-size: 0.82rem;
  backdrop-filter: blur(8px);
  -webkit-backdrop-filter: blur(8px);
}

.chat-msg__text :deep(pre) {
  background: var(--bg-raised);
  border-radius: var(--radius-sm);
  padding: 10px 12px;
  overflow-x: auto;
  margin: 8px 0;
  font-size: 0.82rem;
}
.chat-msg__text :deep(code) {
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.85em;
}
.chat-msg__text :deep(pre code) {
  background: none;
  padding: 0;
}
.chat-msg--user .chat-msg__text :deep(code) {
  background: rgba(255,255,255,0.15);
  padding: 1px 4px;
  border-radius: 3px;
}
.chat-msg--assistant .chat-msg__text :deep(code) {
  background: var(--bg-raised);
  padding: 1px 4px;
  border-radius: 3px;
}

/* Markdown rich content */
.chat-msg__text :deep(h1),
.chat-msg__text :deep(h2),
.chat-msg__text :deep(h3),
.chat-msg__text :deep(h4) {
  margin: 12px 0 6px;
  font-weight: 600;
  line-height: 1.3;
}
.chat-msg__text :deep(h1) { font-size: 1.3em; }
.chat-msg__text :deep(h2) { font-size: 1.15em; }
.chat-msg__text :deep(h3) { font-size: 1.05em; }
.chat-msg__text :deep(ul),
.chat-msg__text :deep(ol) {
  margin: 6px 0;
  padding-left: 1.5em;
}
.chat-msg__text :deep(li) {
  margin: 2px 0;
}
.chat-msg__text :deep(blockquote) {
  border-left: 3px solid var(--border-subtle);
  margin: 8px 0;
  padding: 4px 12px;
  opacity: 0.85;
}
.chat-msg__text :deep(table) {
  border-collapse: collapse;
  margin: 8px 0;
  font-size: 0.85em;
  width: 100%;
}
.chat-msg__text :deep(th),
.chat-msg__text :deep(td) {
  border: 1px solid var(--border-subtle);
  padding: 5px 10px;
  text-align: left;
}
.chat-msg__text :deep(th) {
  background: var(--bg-raised);
  font-weight: 600;
}
.chat-msg__text :deep(a) {
  color: var(--accent);
  text-decoration: underline;
  text-decoration-style: dotted;
}
.chat-msg__text :deep(a:hover) {
  text-decoration-style: solid;
}
.chat-msg__text :deep(hr) {
  border: none;
  border-top: 1px solid var(--border-subtle);
  margin: 10px 0;
}
.chat-msg__text :deep(img) {
  max-width: 100%;
  border-radius: var(--radius-sm);
  margin: 6px 0;
}
.chat-msg__text :deep(p) {
  margin: 4px 0;
}
.chat-msg__text :deep(> p:first-child) {
  margin-top: 0;
}
.chat-msg__text :deep(> p:last-child) {
  margin-bottom: 0;
}

/* Steps timeline */
.chat-steps {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 10px;
  padding-left: 14px;
  border-left: 2px solid var(--border-subtle);
}

/* Step appear animation */
.chat-step {
  animation: step-appear 0.25s ease-out;
}
@keyframes step-appear {
  from { opacity: 0; transform: translateY(-6px); }
  to { opacity: 1; transform: translateY(0); }
}

/* Thinking step — single line to prevent layout jumps */
.step-thinking {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.78rem;
  color: var(--text-muted);
  font-style: italic;
  padding: 3px 0;
  line-height: 1.3;
  height: 24px;
}
.step-thinking__icon {
  width: 12px;
  height: 12px;
  flex-shrink: 0;
  border-radius: 50%;
  background: var(--text-muted);
  opacity: 0.4;
  animation: pulse-think 2s ease-in-out infinite;
}
@keyframes pulse-think {
  0%, 100% { opacity: 0.3; }
  50% { opacity: 0.7; }
}
.step-thinking__text {
  flex: 1;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* Tool step */
.step-tool {
  font-size: 0.8rem;
  border-radius: var(--radius-sm);
  background: var(--bg-raised);
  overflow: hidden;
}
.step-tool__header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  cursor: pointer;
  list-style: none;
  user-select: none;
  transition: background var(--duration-fast) var(--ease-out);
}
.step-tool__header::-webkit-details-marker { display: none; }
.step-tool__header:hover {
  background: var(--bg-surface-hover);
}

/* Tool status dot */
.step-tool__dot {
  width: 7px;
  height: 7px;
  flex-shrink: 0;
  border-radius: 50%;
  transition: background 0.3s var(--ease-out);
}
.step-tool__dot--done {
  background: var(--lobster-success, #22c55e);
}
.step-tool__dot--error {
  background: #ef4444;
}
.step-tool__dot--running {
  background: linear-gradient(90deg, rgba(255,255,255,0.08) 0%, rgba(255,255,255,0.3) 50%, rgba(255,255,255,0.08) 100%);
  background-size: 200% 100%;
  animation: shimmer-dot 1.5s ease-in-out infinite;
}
@keyframes shimmer-dot {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}

.step-tool__name {
  font-weight: 600;
  color: var(--text-primary);
}
.step-tool__raw-name {
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  color: var(--text-muted);
  font-size: 0.72rem;
}
.step-tool-wrap {
  display: flex;
  flex-direction: column;
}
.step-tool__progress {
  padding: 0 10px 2px 22px;
  display: flex;
  flex-direction: column;
  gap: 1px;
}
.step-tool__progress-line {
  font-size: 0.72rem;
  color: var(--text-muted);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  line-height: 1.4;
}
.step-tool__progress-line::before {
  content: '· ';
  opacity: 0.5;
}

.step-tool__body {
  padding: 0 10px 8px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.step-tool__section-label {
  font-size: 0.7rem;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 2px;
}
.step-tool__pre {
  margin: 0;
  padding: 6px 8px;
  background: var(--bg-base);
  border-radius: var(--radius-sm);
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.75rem;
  line-height: 1.4;
  color: var(--text-secondary);
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 200px;
  overflow-y: auto;
}

/* Typing dots */
.chat-typing {
  display: flex; gap: 4px; padding: 4px 0;
}
.chat-typing span {
  width: 6px; height: 6px;
  border-radius: 50%;
  background: var(--text-muted);
  animation: typing-dot 1.4s infinite;
}
.chat-typing span:nth-child(2) { animation-delay: 0.2s; }
.chat-typing span:nth-child(3) { animation-delay: 0.4s; }
@keyframes typing-dot {
  0%, 60%, 100% { opacity: 0.3; transform: translateY(0); }
  30% { opacity: 1; transform: translateY(-3px); }
}

/* Input area — unified container like Claude Code */
.chat-input-area {
  margin: 0 12px 12px;
  background: var(--bg-raised, var(--sidebar-bg));
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  border: 1px solid var(--sidebar-border);
  border-radius: var(--radius-lg);
  overflow: hidden;
  transition: border-color var(--duration-fast) var(--ease-out);
}
.chat-input-area:focus-within {
  border-color: var(--input-focus-border);
}

.chat-disconnected {
  font-size: 0.78rem;
  color: var(--lobster-accent);
  padding: 6px 14px 0;
  text-align: center;
}
.chat-disconnected--connecting { color: var(--text-muted); }
.chat-disconnected--disconnecting { color: var(--lobster-accent); }

.chat-input {
  width: 100%;
  padding: 12px 14px 4px;
  background: transparent;
  border: none;
  color: var(--text-primary);
  font-family: inherit;
  font-size: 0.88rem;
  resize: none;
  outline: none;
  line-height: 1.5;
  height: 42px;
  max-height: 200px;
  overflow-y: hidden;
}
.chat-input::placeholder {
  color: var(--text-muted);
}

/* Footer row: context indicator + send button */
.chat-input-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 8px 8px 14px;
}
.chat-input-footer__left {
  display: flex;
  align-items: center;
  gap: 10px;
}

/* Attach button */
.attach-btn {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  padding: 2px;
  opacity: 0.6;
  transition: opacity 0.2s, color 0.2s;
}
.attach-btn:hover { opacity: 1; color: var(--text-primary); }
.attach-btn:disabled { opacity: 0.3; cursor: not-allowed; }

/* Attached file previews */
.attached-files {
  display: flex;
  gap: 6px;
  padding: 6px 12px 2px;
  flex-wrap: wrap;
}
.attached-file {
  position: relative;
  border-radius: 6px;
  overflow: hidden;
  border: 1px solid var(--border-subtle);
}
.attached-file--img {
  width: 60px;
  height: 60px;
}
.attached-file--img img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.attached-file__info {
  display: flex;
  flex-direction: column;
  padding: 6px 24px 6px 8px;
  gap: 2px;
  min-width: 80px;
  max-width: 160px;
}
.attached-file__name {
  font-size: 0.72rem;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.attached-file__size {
  font-size: 0.65rem;
  color: var(--text-muted);
}
.attached-file__remove {
  position: absolute;
  top: -1px;
  right: -1px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: rgba(0,0,0,0.65);
  color: #fff;
  border: none;
  font-size: 12px;
  line-height: 1;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* User message images */
.chat-msg__images {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  margin-bottom: 6px;
}
.chat-msg__image {
  max-width: 200px;
  max-height: 200px;
  border-radius: 6px;
  object-fit: cover;
}

/* Context indicator: dot + text */
.context-indicator {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: default;
}
.context-dot {
  width: 7px; height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
  transition: background 0.4s var(--ease-out);
}
.context-dot--ok { background: var(--lobster-success, #22c55e); }
.context-dot--moderate { background: var(--lobster-primary, #f97316); }
.context-dot--warning { background: #eab308; }
.context-dot--critical { background: #ef4444; }
.context-text {
  font-size: 0.72rem;
  color: var(--text-muted);
  white-space: nowrap;
}

.chat-send {
  width: 30px; height: 30px;
  border-radius: var(--radius-sm);
  border: none;
  background: var(--lobster-primary);
  color: white;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  transition: all var(--duration-fast) var(--ease-out);
}
.chat-send:disabled {
  opacity: 0.3;
  cursor: not-allowed;
}
.chat-send:not(:disabled):hover {
  background: var(--lobster-primary-hover);
  transform: scale(1.05);
}
.chat-send--stop {
  background: var(--status-err);
}
.chat-send--stop:not(:disabled):hover {
  background: var(--status-err);
  transform: scale(1.05);
}
.chat-send--interrupt {
  background: var(--lobster-accent);
}
.chat-send--interrupt:not(:disabled):hover {
  background: var(--lobster-accent);
  transform: scale(1.05);
}
.stop-icon {
  width: 12px; height: 12px;
  background: white;
  border-radius: 2px;
}

/* Streaming cursor */
.streaming-cursor {
  display: inline-block;
  width: 2px;
  height: 1em;
  background: var(--lobster-primary);
  margin-left: 2px;
  vertical-align: text-bottom;
  animation: cursor-blink 0.8s steps(2) infinite;
}
@keyframes cursor-blink {
  0% { opacity: 1; }
  50% { opacity: 0; }
}


</style>
