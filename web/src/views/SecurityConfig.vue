<template>
  <div>
    <h1 class="page-title">{{ t('security.title') }}</h1>
    <p class="page-desc">{{ t('security.desc') }}</p>
    <p class="page-hint">{{ isZh
      ? '权限通过聊天卡片即时授权（允许一次 / 允许并记住 / 拒绝）。下方是底层防御措施与已记住的授权。'
      : 'Permission is granted interactively via the chat action card (allow once / allow & remember / deny). Below are the defense-in-depth limits and remembered grants.'
    }}</p>

    <div class="max-w-xl space-y-5">
      <!-- Detailed Settings -->
      <GlassCard :hoverable="false">
        <label class="field-label">{{ t('security.detailedSettings') }}</label>

        <div class="setting-row">
          <GlassToggle v-model="form.workspaceOnly" :label="t('security.workspaceOnly')" />
          <span class="setting-hint">{{ t('security.workspaceOnlyHint') }}</span>
        </div>

        <div class="mt-4">
          <GlassTag
            v-model="form.allowedCommands"
            :label="t('security.allowedCommands')"
            placeholder="e.g. git, ls, cat..."
          />
        </div>

        <div class="mt-4">
          <GlassTag
            v-model="form.forbiddenPaths"
            :label="t('security.forbiddenPaths')"
            placeholder="e.g. /etc, C:\\Windows..."
          />
        </div>
      </GlassCard>

      <!-- Approved tools: entries saved via the action card's "Allow & remember". -->
      <GlassCard :hoverable="false">
        <label class="field-label">{{ isZh ? '已批准操作' : 'Approved tools' }}</label>
        <p class="approved-hint">{{ isZh
          ? '通过聊天卡片的"允许并记住"保存的条目。删除后保存并重启 Plaw 生效。'
          : 'Entries saved via the action card\'s "Allow & remember". Delete + Save + Restart Plaw to take effect.'
        }}</p>
        <div v-if="!form.autoApprove.length" class="approved-empty">
          {{ isZh ? '尚无已批准的操作。' : 'No approved tools yet.' }}
        </div>
        <ul v-else class="approved-list">
          <li v-for="(entry, idx) in form.autoApprove" :key="entry + ':' + idx" class="approved-item">
            <code class="approved-entry">{{ entry }}</code>
            <span class="approved-kind">{{ entryKind(entry) }}</span>
            <button class="approved-delete" :title="isZh ? '移除' : 'Remove'"
              @click="form.autoApprove.splice(idx, 1)">×</button>
          </li>
        </ul>
      </GlassCard>

    </div>

    <!-- Sticky save bar -->
    <div class="sticky-actions">
      <div v-if="needRestart" class="restart-bar mb-3">
        <span>{{ t('common.restartHint') }}</span>
        <GlassButton size="sm" variant="primary" :loading="restarting" @click="doRestart">
          {{ t('common.restart') }}
        </GlassButton>
      </div>
      <div class="flex items-center justify-end gap-3">
        <span v-if="saveMsg" class="save-msg" :class="saveOk ? 'save-msg--ok' : 'save-msg--err'">
          {{ saveMsg }}
        </span>
        <GlassButton variant="primary" :loading="saving" @click="save">
          {{ t('common.save') }}
        </GlassButton>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, reactive, onMounted } from 'vue'
import { GlassCard, GlassButton, GlassToggle, GlassTag } from '../components/glass'
import { readConfig, writeConfig, restartPlaw, getPlawStatus } from '../api/tauri'
import { useI18n } from '../composables/useI18n'
const { t, isZh } = useI18n()

const saving = ref(false)
const saveMsg = ref('')
const saveOk = ref(false)
const needRestart = ref(false)
const restarting = ref(false)

// Stage 6 collapse: per-operation approval moved entirely into the action
// card (allow once / allow & remember / deny). The autonomy tier preset
// grid is gone; this page now only edits the defense-in-depth limits
// (workspace_only, allowed_commands, forbidden_paths) and the remembered
// grants (auto_approve).
const form = reactive({
  workspaceOnly: true,
  allowedCommands: [],
  forbiddenPaths: [],
  autoApprove: [],
})

/**
 * Classify an auto_approve entry for display: shell command-prefix grants
 * are stored as "shell:<prefix>"; everything else is a whole-tool grant.
 */
function entryKind(entry) {
  if (typeof entry === 'string' && entry.startsWith('shell:') && entry.length > 6) {
    return isZh.value ? 'shell 前缀' : 'shell prefix'
  }
  return isZh.value ? '整个工具' : 'whole tool'
}

onMounted(async () => {
  try {
    const cfg = await readConfig()
    if (cfg.autonomy) {
      form.workspaceOnly = cfg.autonomy.workspace_only !== false
      form.allowedCommands = cfg.autonomy.allowed_commands || []
      form.forbiddenPaths = cfg.autonomy.forbidden_paths || []
      form.autoApprove = Array.isArray(cfg.autonomy.auto_approve)
        ? [...cfg.autonomy.auto_approve]
        : []
    }
  } catch (e) {
    // Surface to console so a missing/malformed config doesn't masquerade
    // as a UI bug.
    console.warn('[SecurityConfig] readConfig failed:', e)
  }
})

async function save() {
  saving.value = true
  try {
    // Send ONLY the autonomy section we edit on this page. The Tauri
    // write_config command deep-merges this partial into the on-disk
    // config, so other sections (web_fetch, agent, browser, ...) are
    // preserved untouched. This avoids round-tripping large integer
    // defaults (e.g. plaw's [agent].max_tool_iterations near i64::MAX)
    // through JS Number, which loses precision and then fails toml's
    // i64-typed deserialize on the way back.
    await writeConfig({
      autonomy: {
        workspace_only: form.workspaceOnly,
        allowed_commands: form.allowedCommands || [],
        forbidden_paths: form.forbiddenPaths || [],
        auto_approve: form.autoApprove || [],
      },
    })
    saveOk.value = true
    saveMsg.value = t('common.saved')
    try {
      const status = await getPlawStatus()
      if (status) needRestart.value = true
    } catch {}
  } catch (e) {
    saveOk.value = false
    // Tauri Result<_, String> errors arrive as plain strings, not Error
    // objects — `e.message` was always undefined before, so the user saw
    // only the generic fallback. Coerce string/object/null so the real
    // Rust error surfaces in the toast.
    const detail = (e && typeof e === 'object' && 'message' in e)
      ? e.message
      : (typeof e === 'string' ? e : String(e ?? ''))
    console.error('[SecurityConfig] save failed:', e)
    saveMsg.value = detail || t('common.saveFailed')
  } finally {
    saving.value = false
    setTimeout(() => { saveMsg.value = '' }, 5000)
  }
}

async function doRestart() {
  restarting.value = true
  try {
    await restartPlaw()
    needRestart.value = false
  } catch { /* ignore */ }
  finally { restarting.value = false }
}
</script>

<style scoped>
.page-title { margin-bottom: 4px; }
.page-desc { margin-bottom: 8px; }
.page-hint {
  color: var(--text-muted);
  font-size: 0.8rem;
  line-height: 1.5;
  margin-bottom: 20px;
}
.field-label {
  display: block;
  font-size: 0.8rem; font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 12px;
}

.setting-row {
  display: flex; align-items: center; gap: 12px;
  margin-top: 8px;
}
.setting-hint {
  font-size: 0.78rem;
  color: var(--text-muted);
}

.approved-hint {
  font-size: 0.78rem;
  color: var(--text-muted);
  margin: 0 0 10px;
  line-height: 1.5;
}
.approved-empty {
  font-size: 0.82rem;
  color: var(--text-muted);
  font-style: italic;
  padding: 6px 2px;
}
.approved-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.approved-item {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 10px;
  background: var(--bg-raised);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
}
.approved-entry {
  flex: 1;
  font-family: 'Cascadia Code', 'Fira Code', monospace;
  font-size: 0.82rem;
  color: var(--text-primary);
  word-break: break-all;
}
.approved-kind {
  font-size: 0.72rem;
  color: var(--text-muted);
  flex-shrink: 0;
}
.approved-delete {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  background: transparent;
  color: var(--text-muted);
  font-size: 1rem;
  line-height: 1;
  cursor: pointer;
  transition: all var(--duration-fast) var(--ease-out);
}
.approved-delete:hover {
  border-color: var(--status-err, var(--plaw-accent));
  color: var(--status-err, var(--plaw-accent));
}


</style>
