import { ref } from 'vue'

/**
 * Global notification store (singleton).
 * Manages in-app toast queue for cron results and other notifications.
 */

const toasts = ref([])
let _idCounter = 0

/** Add a toast notification */
function addToast({ title, body, sessionId = null, jobId = null, type = 'info', duration = 8000 }) {
  const id = ++_idCounter
  const toast = { id, title, body, sessionId, jobId, type, createdAt: Date.now() }
  toasts.value.push(toast)

  if (duration > 0) {
    setTimeout(() => dismissToast(id), duration)
  }
  return id
}

/** Dismiss a toast by id */
function dismissToast(id) {
  const idx = toasts.value.findIndex(t => t.id === id)
  if (idx !== -1) toasts.value.splice(idx, 1)
}

/** Clear all toasts */
function clearAll() {
  toasts.value = []
}

/** Callback for toast click — set by App.vue */
let _onClickHandler = null

function setOnClick(handler) {
  _onClickHandler = handler
}

function handleClick(toast) {
  if (_onClickHandler) _onClickHandler(toast)
  dismissToast(toast.id)
}

export function useNotifications() {
  return { toasts, addToast, dismissToast, clearAll, setOnClick, handleClick }
}
