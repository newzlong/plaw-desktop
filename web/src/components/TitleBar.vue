<template>
  <div class="titlebar" data-tauri-drag-region>
    <div class="titlebar__left">
      <button class="titlebar__tool" @click="openSettings()" title="Settings">
        <SettingsIcon :size="13" />
      </button>
      <button class="titlebar__tool" @click="toggleTheme" :title="isDark ? 'Light' : 'Dark'">
        <component :is="isDark ? Sun : Moon" :size="13" />
      </button>
      <button class="titlebar__tool titlebar__lang" @click="toggleLocale">
        {{ isZh ? 'EN' : '中' }}
      </button>
    </div>
    <div class="titlebar__controls">
      <button class="titlebar__btn" @click="minimize" title="Minimize">
        <Minus :size="13" />
      </button>
      <button class="titlebar__btn" @click="toggleMaximize" title="Maximize">
        <component :is="isMaximized ? Minimize2 : Maximize2" :size="13" />
      </button>
      <button class="titlebar__btn titlebar__btn--close" @click="close" title="Close">
        <X :size="13" />
      </button>
    </div>
  </div>
</template>

<script setup>
import { ref, inject, onMounted, onUnmounted } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Minus, Maximize2, Minimize2, X, Sun, Moon, Settings as SettingsIcon } from 'lucide-vue-next'

const toggleTheme = inject('toggleTheme')
const toggleLocale = inject('toggleLocale')
const isDark = inject('isDark')
const isZh = inject('isZh')

const openSettings = inject('openSettings')

const isMaximized = ref(false)
let unlisten = null

async function minimize() {
  await getCurrentWindow().minimize()
}

async function toggleMaximize() {
  const win = getCurrentWindow()
  await win.toggleMaximize()
}

async function close() {
  await getCurrentWindow().close()
}

onMounted(async () => {
  const win = getCurrentWindow()
  isMaximized.value = await win.isMaximized()
  unlisten = await win.onResized(async () => {
    isMaximized.value = await win.isMaximized()
  })
})

onUnmounted(() => {
  if (unlisten) unlisten()
})
</script>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 32px;
  flex-shrink: 0;
  background: var(--card-bg);
  border-bottom: 1px solid var(--border-subtle);
  user-select: none;
  -webkit-app-region: drag;
}

.titlebar__left {
  display: flex;
  align-items: center;
  padding-left: 8px;
  gap: 2px;
  -webkit-app-region: no-drag;
}

.titlebar__tool {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 24px;
  background: transparent;
  border: none;
  border-radius: 4px;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 0.72rem;
  transition: background 0.15s, color 0.15s;
}
.titlebar__tool:hover {
  background: rgba(255, 255, 255, 0.08);
  color: var(--text-primary);
}

.titlebar__lang {
  font-weight: 600;
  font-size: 0.7rem;
}

.titlebar__controls {
  display: flex;
  align-items: center;
  padding-right: 8px;
  gap: 2px;
  -webkit-app-region: no-drag;
}

.titlebar__btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 24px;
  background: transparent;
  border: none;
  border-radius: 4px;
  color: var(--text-muted);
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}
.titlebar__btn:hover {
  background: rgba(255, 255, 255, 0.08);
  color: var(--text-primary);
}
.titlebar__btn--close:hover {
  background: #e81123;
  color: #fff;
}
</style>
