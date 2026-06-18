<template>
  <div>
    <div class="page-header">
      <div>
        <h1 class="page-title">{{ t('skills.title') }}</h1>
        <p class="page-desc">{{ t('skills.desc') }}</p>
      </div>
      <div class="flex items-center gap-2">
        <GlassButton size="sm" variant="ghost" @click="refreshLocal">
          <RefreshCw class="w-4 h-4" />
        </GlassButton>
        <GlassButton size="sm" variant="primary" @click="showInstall = true">
          <Plus class="w-4 h-4" />
          {{ t('skills.installSkill') }}
        </GlassButton>
      </div>
    </div>

    <!-- Tabs -->
    <div class="tabs">
      <button
        class="tab" :class="{ 'tab--active': tab === 'installed' }"
        @click="tab = 'installed'"
      >{{ t('skills.tabInstalled') }} ({{ localSkills.length }})</button>
      <button
        class="tab" :class="{ 'tab--active': tab === 'market' }"
        @click="tab = 'market'; fetchMarket()"
      >{{ t('skills.tabMarket') }}</button>
    </div>

    <!-- Installed tab -->
    <template v-if="tab === 'installed'">
      <!-- Skeleton -->
      <template v-if="localLoading">
        <div class="space-y-3">
          <div v-for="i in 4" :key="i" class="skel-skill-card">
            <div class="skel-skill-info">
              <GlassSkeleton width="120px" height="0.95rem" />
              <GlassSkeleton width="260px" height="0.8rem" class="mt-1.5" />
              <GlassSkeleton width="60px" height="0.7rem" class="mt-2" />
            </div>
            <GlassSkeleton width="52px" height="28px" rounded />
          </div>
        </div>
      </template>
      <template v-else>
      <div v-if="localSkills.length" class="filter-bar">
        <GlassInput
          v-model="installedQuery"
          :placeholder="t('skills.searchPlaceholder')"
          class="filter-bar__input"
        />
        <div class="filter-dots">
          <button
            v-for="f in ['verified', 'needs-setup', 'incompatible', 'untagged']" :key="'c-'+f"
            class="filter-dot-btn"
            :class="{ 'filter-dot-btn--active': compatToggles[f] }"
            :title="compatLabel(f) + ' (' + compatCount(f) + ')'"
            @click="compatToggles[f] = !compatToggles[f]"
          ><span class="compat-dot" :class="'compat-dot--' + f" /></button>
        </div>
        <span class="filter-sep" />
        <div class="filter-dots">
          <button
            v-for="f in ['safe', 'warning', 'danger', 'untagged']" :key="'r-'+f"
            class="filter-dot-btn"
            :class="{ 'filter-dot-btn--active': riskToggles[f] }"
            :title="riskLabel(f) + ' (' + riskCount(f) + ')'"
            @click="riskToggles[f] = !riskToggles[f]"
          ><span class="risk-diamond" :class="'risk-diamond--' + f" /></button>
        </div>
        <span class="filter-sep" />
        <GlassButton
          size="sm" variant="ghost"
          :disabled="auditingAll"
          @click="doAuditAll"
        >
          <Loader2 v-if="auditingAll" :size="13" class="spin" />
          <ShieldCheck v-else :size="13" />
          {{ auditingAll ? t('skills.auditingAll') : t('skills.auditAll') }}
        </GlassButton>
      </div>

      <GlassCard v-if="!localSkills.length" :hoverable="false">
        <div class="empty-hint">
          <Puzzle class="w-5 h-5" style="color: var(--text-muted)" />
          <span>{{ t('skills.empty') }}</span>
        </div>
      </GlassCard>

      <div v-else class="space-y-3">
        <GlassCard v-for="skill in filteredLocalSkills" :key="skill.name" :hoverable="false">
          <div class="skill-row">
            <div class="skill-info">
              <div class="skill-name-row">
                <span class="skill-name">{{ skill.name }}</span>
                <span
                  class="compat-dot"
                  :class="'compat-dot--' + (skill.compatibility || 'untagged')"
                  :title="skill.compatibility ? compatLabel(skill.compatibility) : t('skills.compatUntagged')"
                />
                <span
                  v-if="skill.risk"
                  class="risk-diamond"
                  :class="'risk-diamond--' + skill.risk"
                  :title="riskLabel(skill.risk)"
                />
              </div>
              <div class="skill-desc">{{ skill.description || t('skills.noDesc') }}</div>
              <div class="skill-source">
                <span v-if="skill.source === 'builtin'" class="badge-builtin">{{ t('skills.builtin') }}</span>
                <span v-else>{{ skill.source }}</span>
              </div>
            </div>
            <div class="skill-actions">
              <button
                class="audit-btn"
                :class="{ 'audit-btn--active': auditingSkills.has(skillSlug(skill)) }"
                :title="t('skills.auditTitle')"
                :disabled="auditingSkills.has(skillSlug(skill))"
                @click="runAudit(skillSlug(skill))"
              >
                <Loader2 v-if="auditingSkills.has(skillSlug(skill))" :size="15" class="spin" />
                <ShieldCheck v-else :size="15" />
              </button>
              <GlassButton
                v-if="skill.source === 'managed'"
                size="sm" variant="ghost" class="uninstall-btn"
                @click="doUninstall(skillSlug(skill))"
              >{{ t('common.delete') }}</GlassButton>
            </div>
          </div>
        </GlassCard>
      </div>
      </template><!-- end v-else localLoading -->
    </template>

    <!-- Market tab -->
    <template v-if="tab === 'market'">
      <!-- Source status: offline warning with proxy config -->
      <div v-if="marketSource === 'local' && marketError" class="source-hint source-hint--warn">
        <div class="source-hint__main">
          <AlertTriangle class="w-4 h-4" style="flex-shrink: 0" />
          <span>{{ t('skills.offlineHint') }}</span>
        </div>
        <div class="proxy-config">
          <div class="proxy-config__row">
            <input
              v-model="proxyInput"
              class="proxy-input"
              :placeholder="t('skills.proxyPlaceholder')"
              @keyup.enter="saveProxyAndRetry"
            />
            <GlassButton size="sm" variant="primary" @click="saveProxyAndRetry" :loading="marketLoading">
              {{ t('skills.retryWithProxy') }}
            </GlassButton>
          </div>
          <div class="proxy-config__hint">{{ t('skills.proxyExamples') }}</div>
        </div>
      </div>

      <!-- Source status: online success -->
      <div v-else-if="marketSource === 'online'" class="source-hint source-hint--ok">
        <Globe class="w-4 h-4" style="flex-shrink: 0" />
        <span>{{ t('skills.onlineHint') }}</span>
        <button v-if="savedProxy" class="proxy-clear" @click="showProxyEdit = !showProxyEdit">
          <Settings class="w-3.5 h-3.5" />
        </button>
      </div>
      <!-- Proxy edit panel (toggled by settings icon) -->
      <div v-if="marketSource === 'online' && showProxyEdit" class="proxy-edit-panel">
        <div class="proxy-config__row">
          <input
            v-model="proxyInput"
            class="proxy-input"
            :placeholder="t('skills.proxyPlaceholder')"
            @keyup.enter="saveProxyAndRetry"
          />
          <GlassButton size="sm" variant="primary" @click="saveProxyAndRetry" :loading="marketLoading">
            {{ t('common.save') }}
          </GlassButton>
          <GlassButton size="sm" variant="ghost" @click="clearProxy">
            {{ t('common.clear') }}
          </GlassButton>
        </div>
      </div>

      <GlassInput
        v-model="marketQuery"
        :placeholder="t('skills.searchPlaceholder')"
        class="mb-4"
      />

      <GlassCard v-if="marketLoading" :hoverable="false">
        <div class="empty-hint">
          <Loader2 class="w-5 h-5 spin" />
          <span>{{ t('skills.loading') }}</span>
        </div>
      </GlassCard>

      <GlassCard v-else-if="!marketSkills.length" :hoverable="false">
        <div class="empty-hint">
          <Search class="w-5 h-5" style="color: var(--text-muted)" />
          <span>{{ t('skills.marketEmpty') }}</span>
        </div>
      </GlassCard>

      <div v-else class="space-y-3">
        <GlassCard v-for="skill in marketSkills" :key="skill.name" :hoverable="false">
          <div class="skill-row">
            <div class="skill-info">
              <div class="skill-name">{{ skill.name }}</div>
              <div class="skill-desc">{{ skill.description || t('skills.noDesc') }}</div>
              <div v-if="skill.author" class="skill-source">{{ skill.author }}</div>
            </div>
            <GlassButton
              size="sm"
              :variant="isInstalled(skill.name) ? 'ghost' : 'primary'"
              :disabled="isInstalled(skill.name)"
              :loading="installingSet.has(skill.name)"
              @click="doInstallFromMarket(skill)"
            >{{ isInstalled(skill.name) ? t('skills.installed') : t('common.install') }}</GlassButton>
          </div>
        </GlassCard>
      </div>
    </template>

    <!-- Install dialog (manual path/URL) -->
    <GlassDialog v-model="showInstall" :title="t('skills.installTitle')">
      <div class="dialog-form">
        <p class="dialog-hint">{{ t('skills.installDesc') }}</p>
        <GlassInput
          v-model="installPath"
          :label="t('skills.pathOrUrl')"
          placeholder="/path/to/skill or https://..."
        />
      </div>
      <template #footer>
        <GlassButton variant="ghost" @click="showInstall = false">{{ t('common.cancel') }}</GlassButton>
        <GlassButton variant="primary" @click="doInstallManual" :loading="installing" :disabled="!installPath">
          {{ t('common.install') }}
        </GlassButton>
      </template>
    </GlassDialog>

    <!-- AI Audit dialog -->
    <GlassDialog v-model="showAudit" :title="t('skills.auditTitle')" :closable="!auditing" persistent>
      <div class="audit-content">
        <!-- Auditing state -->
        <div v-if="auditing" class="audit-step">
          <span class="audit-dot audit-dot--running" />
          <span>{{ auditStep }}</span>
        </div>

        <!-- Result state -->
        <template v-if="auditResult && !auditing">
          <div class="audit-result">
            <div class="audit-result__row">
              <span class="audit-label">{{ t('skills.auditCompatibility') }}</span>
              <span class="compat-badge" :class="'compat-badge--' + auditResult.compatibility">
                {{ compatLabel(auditResult.compatibility) }}
              </span>
            </div>
            <div class="audit-result__row">
              <span class="audit-label">{{ t('skills.auditRisk') }}</span>
              <span class="risk-badge" :class="'risk-badge--' + auditResult.risk">
                {{ riskLabel(auditResult.risk) }}
              </span>
            </div>
            <div class="audit-reason">{{ auditResult.reason }}</div>
            <div v-if="auditResult.dependencies?.length" class="audit-deps">
              <span class="audit-label">{{ t('skills.auditDeps') }}</span>
              <span v-for="dep in auditResult.dependencies" :key="dep" class="dep-tag">{{ dep }}</span>
            </div>
          </div>

          <!-- Incompatible warning -->
          <div v-if="auditResult.compatibility === 'incompatible'" class="audit-warn">
            <AlertTriangle class="w-4 h-4" />
            <span>{{ t('skills.auditIncompatWarn') }}</span>
          </div>
        </template>

        <!-- Error state -->
        <div v-if="auditError" class="audit-error">
          <span>{{ auditError }}</span>
        </div>
      </div>

      <template #footer>
        <template v-if="!auditing && auditResult">
          <GlassButton
            v-if="auditResult.compatibility === 'incompatible'"
            variant="ghost"
            @click="doUninstallAudited"
          >{{ t('skills.auditUninstall') }}</GlassButton>
          <GlassButton
            v-if="auditResult.compatibility === 'incompatible'"
            variant="primary"
            @click="showAudit = false"
          >{{ t('skills.auditKeep') }}</GlassButton>
          <GlassButton
            v-else
            variant="primary"
            @click="showAudit = false"
          >{{ t('common.ok') }}</GlassButton>
        </template>
        <GlassButton v-if="auditError" variant="ghost" @click="showAudit = false">{{ t('common.close') }}</GlassButton>
      </template>
    </GlassDialog>
  </div>
</template>

<script setup>
import { ref, reactive, computed, onMounted, onUnmounted } from 'vue'
import { Plus, Puzzle, Loader2, Search, AlertTriangle, RefreshCw, Globe, Settings, ShieldCheck } from 'lucide-vue-next'
import { GlassCard, GlassButton, GlassInput, GlassDialog, GlassSkeleton } from '../components/glass'
import { useI18n } from '../composables/useI18n'
import {
  listLocalSkills, installSkill, uninstallSkill, auditSkill, auditAllUnaudited, searchRegistrySkills,
  syncSkillsRegistry, getMarketProxy, setMarketProxy,
} from '../api/tauri'

const { t } = useI18n()

const tab = ref('installed')
const localLoading = ref(true)
const localSkills = ref([])
const installedQuery = ref('')
const showInstall = ref(false)
const installPath = ref('')
const installing = ref(false)
const installingSet = reactive(new Set())

// Batch audit state
const auditingAll = ref(false)
const auditingSkills = reactive(new Set())
let unlistenAudit = null

async function doAuditAll() {
  auditingAll.value = true
  try {
    // Mark all eligible skills as auditing
    const BUILTIN = new Set(['find-skills', 'skill-creator', 'audit-skills', 'fix-skills', 'pptx', 'xlsx', 'docx', 'pdf'])
    for (const s of localSkills.value) {
      if (!BUILTIN.has(s.name)) auditingSkills.add(skillSlug(s))
    }
    const count = await auditAllUnaudited(true)
    if (count === 0) {
      auditingSkills.clear()
      auditingAll.value = false
    }
    // Completion is handled by the skill-audited event listener
  } catch {
    auditingSkills.clear()
    auditingAll.value = false
  }
}

// Audit state
const showAudit = ref(false)
const auditing = ref(false)
const auditStep = ref('')
const auditResult = ref(null)
const auditError = ref('')
const auditedSkillName = ref('')

async function runAudit(skillName) {
  showAudit.value = true
  auditing.value = true
  auditResult.value = null
  auditError.value = ''
  auditedSkillName.value = skillName
  auditStep.value = t('skills.auditStep1')

  try {
    // Small delay so user sees the step
    await new Promise(r => setTimeout(r, 500))
    auditStep.value = t('skills.auditStep2')
    const result = await auditSkill(skillName)
    auditResult.value = result
    // Refresh list so the dot color updates
    refreshLocal()
  } catch (e) {
    auditError.value = typeof e === 'string' ? e : (e?.message || 'Audit failed')
  } finally {
    auditing.value = false
  }
}

async function doUninstallAudited() {
  if (auditedSkillName.value) {
    try {
      await uninstallSkill(auditedSkillName.value)
      await refreshLocal()
    } catch {}
  }
  showAudit.value = false
}

const compatToggles = reactive({ verified: true, 'needs-setup': true, incompatible: true, untagged: true })
const riskToggles = reactive({ safe: true, warning: true, danger: true, untagged: true })

function skillSlug(skill) {
  // Extract directory name from path (e.g. "...skills/agent-browser" -> "agent-browser")
  if (skill.path) {
    const parts = skill.path.replace(/\\/g, '/').split('/')
    return parts[parts.length - 1] || skill.name
  }
  return skill.name
}

function compatLabel(tag) {
  const labels = {
    verified: t('skills.compatVerified'),
    'needs-setup': t('skills.compatNeedsSetup'),
    incompatible: t('skills.compatIncompatible'),
    untagged: t('skills.compatUntagged'),
  }
  return labels[tag] || tag
}

function compatCount(tag) {
  if (tag === 'untagged') return localSkills.value.filter(s => !s.compatibility).length
  return localSkills.value.filter(s => s.compatibility === tag).length
}

function riskLabel(tag) {
  const labels = {
    safe: t('skills.riskSafe'),
    warning: t('skills.riskWarning'),
    danger: t('skills.riskDanger'),
    untagged: t('skills.riskUntagged'),
  }
  return labels[tag] || tag
}

function riskCount(tag) {
  if (tag === 'untagged') return localSkills.value.filter(s => !s.risk).length
  return localSkills.value.filter(s => s.risk === tag).length
}

const filteredLocalSkills = computed(() => {
  let list = localSkills.value
  // Filter by toggled compatibility categories
  const allCompatOn = compatToggles.verified && compatToggles['needs-setup'] && compatToggles.incompatible && compatToggles.untagged
  if (!allCompatOn) {
    list = list.filter(s => {
      const c = s.compatibility || ''
      if (!c) return compatToggles.untagged
      if (c === 'verified') return compatToggles.verified
      if (c === 'needs-setup') return compatToggles['needs-setup']
      if (c === 'incompatible') return compatToggles.incompatible
      return compatToggles.untagged
    })
  }
  // Filter by toggled risk categories
  const allRiskOn = riskToggles.safe && riskToggles.warning && riskToggles.danger && riskToggles.untagged
  if (!allRiskOn) {
    list = list.filter(s => {
      const r = s.risk || ''
      if (!r) return riskToggles.untagged
      if (r === 'safe') return riskToggles.safe
      if (r === 'warning') return riskToggles.warning
      if (r === 'danger') return riskToggles.danger
      return riskToggles.untagged
    })
  }
  const q = installedQuery.value.toLowerCase().trim()
  if (q) list = list.filter(s =>
    s.name.toLowerCase().includes(q) || (s.description || '').toLowerCase().includes(q)
  )
  return list
})

// Market
const marketQuery = ref('')
const allMarketSkills = ref([])  // full list from API
const marketLoading = ref(false)
const marketSource = ref('')  // "online" or "local"
const marketError = ref('')   // GitHub error message if fallback
const syncing = ref(false)

const marketSkills = computed(() => {
  const q = marketQuery.value.toLowerCase().trim()
  return allMarketSkills.value.filter(s =>
    !isInstalled(s.name) && (!q || s.name.toLowerCase().includes(q))
  )
})

// Proxy
const proxyInput = ref('')
const savedProxy = ref('')
const showProxyEdit = ref(false)

async function refreshLocal() {
  try {
    localSkills.value = await listLocalSkills()
  } finally {
    localLoading.value = false
  }
}

function isInstalled(name) {
  return localSkills.value.some(s => s.name === name)
}

async function doInstallManual() {
  installing.value = true
  try {
    const name = await installSkill(installPath.value)
    showInstall.value = false
    installPath.value = ''
    await refreshLocal()
    runAudit(name)
  } catch (e) {
    alert(typeof e === 'string' ? e : (e?.message || 'Install failed'))
  } finally {
    installing.value = false
  }
}

async function doInstallFromMarket(skill) {
  if (installingSet.has(skill.name)) return
  installingSet.add(skill.name)
  try {
    const name = await installSkill(skill.url || skill.name)
    await refreshLocal()
    runAudit(name)
  } catch (e) {
    alert(typeof e === 'string' ? e : (e?.message || 'Install failed'))
  } finally {
    installingSet.delete(skill.name)
  }
}

async function doUninstall(name) {
  try {
    await uninstallSkill(name)
    await refreshLocal()
  } catch (e) {
    alert(typeof e === 'string' ? e : (e?.message || 'Uninstall failed'))
  }
}

async function fetchMarket() {
  if (allMarketSkills.value.length && marketSource.value) return  // already loaded
  marketLoading.value = true
  try {
    const result = await searchRegistrySkills('')
    allMarketSkills.value = result.skills || result
    marketSource.value = result.source || 'unknown'
    marketError.value = result.error || ''
  } catch {
    allMarketSkills.value = []
    marketSource.value = ''
    marketError.value = ''
  } finally {
    marketLoading.value = false
  }
}

async function refetchMarket() {
  allMarketSkills.value = []
  marketSource.value = ''
  await fetchMarket()
}

async function saveProxyAndRetry() {
  try {
    await setMarketProxy(proxyInput.value.trim())
    savedProxy.value = proxyInput.value.trim()
    await refetchMarket()
  } catch (e) {
    alert(typeof e === 'string' ? e : (e?.message || 'Failed'))
  }
}

async function clearProxy() {
  proxyInput.value = ''
  savedProxy.value = ''
  await setMarketProxy('')
  await refetchMarket()
}

async function doSync() {
  syncing.value = true
  try {
    const count = await syncSkillsRegistry()
    alert(t('skills.syncSuccess').replace('{count}', count))
    await refetchMarket()
  } catch (e) {
    const msg = typeof e === 'string' ? e : (e?.message || String(e))
    alert(t('skills.syncFail').replace('{error}', msg))
  } finally {
    syncing.value = false
  }
}

onMounted(async () => {
  refreshLocal()
  savedProxy.value = await getMarketProxy()
  proxyInput.value = savedProxy.value
  // Listen for per-skill audit completion events
  try {
    const { listen } = await import('@tauri-apps/api/event')
    unlistenAudit = await listen('skill-audited', (event) => {
      const { name, success } = event.payload || {}
      if (name) {
        auditingSkills.delete(name)
        refreshLocal()
      }
      // All done
      if (auditingSkills.size === 0) {
        auditingAll.value = false
      }
    })
  } catch {}
})

onUnmounted(() => {
  if (unlistenAudit) unlistenAudit()
})
</script>

<style scoped>
/* --- Skeleton --- */
.skel-skill-card {
  display: flex; align-items: center; justify-content: space-between;
  padding: 16px 20px;
  background: var(--bg-surface);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-card);
}
.skel-skill-info {
  display: flex; flex-direction: column;
}
.mt-1\.5 { margin-top: 0.375rem; }
.mt-2 { margin-top: 0.5rem; }


.page-desc { margin-top: 4px; }

.tabs {
  display: flex; gap: 2px;
  margin-bottom: 16px;
  background: var(--bg-raised);
  border-radius: var(--radius-md);
  padding: 3px;
  border: 1px solid var(--border-subtle);
}
.tab {
  flex: 1; padding: 8px 16px;
  background: none; border: none;
  color: var(--text-muted); font-size: 0.85rem; font-weight: 500;
  cursor: pointer; border-radius: var(--radius-sm);
  transition: all var(--duration-fast) var(--ease-out);
}
.tab:hover { color: var(--text-primary); }
.tab--active {
  background: var(--bg-surface);
  color: var(--text-primary);
  box-shadow: var(--shadow-card);
}

.empty-hint {
  display: flex; align-items: center; gap: 10px;
  color: var(--text-secondary); font-size: 0.875rem;
}

.skill-row {
  display: flex; align-items: center; justify-content: space-between;
}
.skill-name {
  font-size: 0.95rem; font-weight: 600;
  color: var(--text-primary);
}
.skill-desc {
  font-size: 0.8rem; color: var(--text-muted); margin-top: 2px;
  max-width: 400px;
  overflow: hidden; text-overflow: ellipsis;
  display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical;
}
.skill-source {
  font-size: 0.7rem; color: var(--text-muted); margin-top: 4px;
  text-transform: uppercase; letter-spacing: 0.05em;
}
.badge-builtin {
  display: inline-block;
  padding: 1px 6px;
  border-radius: 3px;
  background: var(--status-info-soft);
  color: var(--status-info);
  font-size: 0.65rem;
  font-weight: 600;
  letter-spacing: 0.04em;
}

.filter-bar {
  display: flex; align-items: center; gap: 8px;
  margin-bottom: 16px;
}
.filter-bar__input { flex: 1; }
.filter-dots {
  display: flex; align-items: center; gap: 4px;
}
.filter-sep {
  width: 1px; height: 14px;
  background: var(--border-subtle); margin: 0 2px;
}
.filter-dot-btn {
  display: flex; align-items: center; justify-content: center;
  width: 20px; height: 20px;
  background: none; border: none; cursor: pointer;
  border-radius: 50%; padding: 0;
  opacity: 0.4;
  transition: all var(--duration-fast) var(--ease-out);
}
.filter-dot-btn:hover { opacity: 0.7; }
.filter-dot-btn--active {
  opacity: 1;
  background: rgba(255,255,255,0.06);
}

.skill-name-row {
  display: flex; align-items: center; gap: 6px;
}
.compat-dot {
  width: 7px; height: 7px; border-radius: 50%;
  flex-shrink: 0;
}
.compat-dot--verified { background: var(--status-ok); }
.compat-dot--needs-setup { background: var(--status-warn); }
.compat-dot--incompatible { background: var(--status-err); }
.compat-dot--untagged { background: rgb(120, 120, 130); opacity: 0.6; }
.skill-actions {
  display: flex; align-items: center; gap: 4px; flex-shrink: 0;
}
.audit-btn {
  background: none; border: none; cursor: pointer;
  color: var(--text-muted); padding: 4px;
  border-radius: var(--radius-sm);
  display: flex; align-items: center; justify-content: center;
  transition: color var(--duration-fast), background var(--duration-fast);
}
.audit-btn:hover:not(:disabled) {
  color: var(--text-primary);
  background: rgba(255, 255, 255, 0.08);
}
.audit-btn--active {
  color: var(--plaw-accent);
  cursor: default;
}

.risk-diamond {
  width: 7px; height: 7px; flex-shrink: 0;
  transform: rotate(45deg);
  border-radius: 1px;
}
.risk-diamond--safe { background: var(--status-ok); }
.risk-diamond--warning { background: var(--status-warn); }
.risk-diamond--danger { background: var(--status-err); }
.risk-diamond--untagged { background: rgb(120, 120, 130); opacity: 0.6; }

/* Audit dialog */
.audit-content { min-height: 60px; }
.audit-step {
  display: flex; align-items: center; gap: 8px;
  color: var(--text-secondary); font-size: 0.85rem;
}
.audit-dot {
  width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0;
}
.audit-dot--running {
  background: var(--plaw-accent);
  animation: shimmer 1.2s ease-in-out infinite;
}
@keyframes shimmer { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }
.audit-result { display: flex; flex-direction: column; gap: 8px; }
.audit-result__row {
  display: flex; align-items: center; gap: 8px;
}
.audit-label {
  font-size: 0.75rem; color: var(--text-muted);
  text-transform: uppercase; letter-spacing: 0.05em;
  min-width: 90px;
}
.compat-badge, .risk-badge {
  font-size: 0.75rem; font-weight: 600;
  padding: 2px 8px; border-radius: 10px;
}
.compat-badge--verified { background: var(--status-ok-soft); color: var(--status-ok); }
.compat-badge--needs-setup { background: var(--status-warn-soft); color: var(--status-warn); }
.compat-badge--incompatible { background: var(--status-err-soft); color: var(--status-err); }
.risk-badge--safe { background: var(--status-ok-soft); color: var(--status-ok); }
.risk-badge--warning { background: var(--status-warn-soft); color: var(--status-warn); }
.risk-badge--danger { background: var(--status-err-soft); color: var(--status-err); }
.audit-reason {
  font-size: 0.8rem; color: var(--text-secondary);
  line-height: 1.4; margin-top: 4px;
}
.audit-deps {
  display: flex; align-items: center; gap: 6px; flex-wrap: wrap;
  margin-top: 4px;
}
.dep-tag {
  font-size: 0.7rem; padding: 1px 6px;
  background: var(--bg-raised); border-radius: 4px;
  color: var(--text-muted); border: 1px solid var(--border-subtle);
}
.audit-warn {
  display: flex; align-items: center; gap: 8px;
  margin-top: 10px; padding: 8px 10px;
  background: var(--status-err-soft); border-radius: var(--radius-sm);
  border: 1px solid rgba(239,68,68,0.2);
  color: var(--status-err); font-size: 0.8rem;
}
.audit-error {
  color: var(--status-err); font-size: 0.8rem;
}

.uninstall-btn { color: var(--status-err) !important; }

.source-hint {
  padding: 12px 14px; margin-bottom: 12px;
  border-radius: var(--radius-sm);
  font-size: 0.8rem;
}
.source-hint--warn {
  background: var(--plaw-accent-soft);
  border: 1px solid rgba(245, 158, 11, 0.2);
  color: var(--plaw-accent);
}
.source-hint--ok {
  display: flex; align-items: center; gap: 8px;
  background: var(--status-ok-soft);
  border: 1px solid rgba(34, 197, 94, 0.2);
  color: var(--status-ok);
}
.source-hint--ok span { flex: 1; }
.source-hint__main {
  display: flex; align-items: center; gap: 8px;
}
.proxy-config {
  margin-top: 10px;
  padding-top: 10px;
  border-top: 1px solid var(--status-warn-soft);
}
.proxy-config__row {
  display: flex; gap: 8px; align-items: center;
}
.proxy-input {
  flex: 1; padding: 6px 10px;
  background: rgba(0, 0, 0, 0.2);
  border: 1px solid rgba(245, 158, 11, 0.25);
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  font-size: 0.8rem; font-family: monospace;
  outline: none;
  transition: border-color var(--duration-fast) var(--ease-out);
}
.proxy-input:focus {
  border-color: var(--plaw-accent);
}
.proxy-input::placeholder {
  color: var(--text-muted);
}
.proxy-config__hint {
  font-size: 0.72rem; color: var(--text-muted);
  margin-top: 6px;
}
.proxy-clear {
  background: none; border: none;
  color: inherit; cursor: pointer;
  opacity: 0.6; padding: 2px;
  transition: opacity var(--duration-fast) var(--ease-out);
}
.proxy-clear:hover { opacity: 1; }


.spin { animation: spin 1s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>
