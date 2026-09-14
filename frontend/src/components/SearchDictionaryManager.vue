<script setup>
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { api } from '../api'
import { vDialog } from '../dialog'

const loading = ref(false)
const statusLoading = ref(false)
const busy = ref(false)
const publishLoading = ref(false)
const error = ref('')
const success = ref('')
const pendingChanges = ref(Number(localStorage.getItem('htknow_lexicon_pending') || 0))
const selectedLexicons = ref([])
const selectedSynonyms = ref([])
const batchProgress = ref(null)
const confirmDialog = ref(null)
const markLexiconChanged = () => {
  pendingChanges.value++
  localStorage.setItem('htknow_lexicon_pending', String(pendingChanges.value))
}

const lexiconQuery = ref('')
const lexiconEnabledFilter = ref('all')
const lexicons = ref([])
const lexiconTotal = ref(0)
const newLexiconTerm = ref('')
const newLexiconFreq = ref('')
const newLexiconTag = ref('')
const newLexiconEnabled = ref(true)

const synonymQuery = ref('')
const synonymEnabledFilter = ref('all')
const synonyms = ref([])
const synonymTotal = ref(0)
const newSynonymTerm = ref('')
const newSynonymValue = ref('')
const newSynonymWeight = ref('1')
const newSynonymBidirectional = ref(true)
const newSynonymEnabled = ref(true)

const editDialog = ref(null)
const editForm = ref({ term: '', synonym: '', freq: '', tag: '', weight: 1, bidirectional: true, enabled: true })

const rebuildStatus = ref({
  job_id: null,
  status: 'idle',
  phase: '',
  total_docs: 0,
  processed_docs: 0,
  progress_pct: 0,
  elapsed_secs: null,
  eta_secs: null,
  updated_at: null,
  started_at: null,
  finished_at: null,
  error: null,
})

let statusPollTimer = null

const resolveEnabledFilter = (value) => {
  if (value === 'enabled') return true
  if (value === 'disabled') return false
  return undefined
}

const progressPercent = computed(() => {
  const pct = Number(rebuildStatus.value?.progress_pct || 0)
  if (Number.isNaN(pct)) return 0
  return Math.max(0, Math.min(100, pct))
})

const progressText = computed(() => {
  const total = Number(rebuildStatus.value?.total_docs || 0)
  const processed = Number(rebuildStatus.value?.processed_docs || 0)
  if (total <= 0) return '0 / 0'
  return `${Math.min(processed, total)} / ${total}`
})

const isRebuilding = computed(() => rebuildStatus.value?.status === 'running')
const statusBadgeClass = computed(() => {
  if (rebuildStatus.value?.status === 'running') return 'bg-amber-50 text-amber-700 border-amber-200'
  if (rebuildStatus.value?.status === 'completed') return 'bg-emerald-50 text-emerald-700 border-emerald-200'
  if (rebuildStatus.value?.status === 'failed') return 'bg-red-50 text-red-700 border-red-200'
  return 'bg-slate-50 text-slate-600 border-slate-200'
})

const formatSecs = (value) => {
  if (value === null || value === undefined) return '-'
  const total = Math.max(0, Number(value))
  if (total < 60) return `${total}s`
  const minutes = Math.floor(total / 60)
  const seconds = total % 60
  if (minutes < 60) return `${minutes}m ${seconds}s`
  const hours = Math.floor(minutes / 60)
  const remMinutes = minutes % 60
  return `${hours}h ${remMinutes}m`
}

const formatTime = (value) => {
  if (!value) return '-'
  const date = new Date(Number(value) * 1000)
  if (Number.isNaN(date.getTime())) return '-'
  return date.toLocaleString()
}

const clearTips = () => {
  error.value = ''
  success.value = ''
}

const refreshStatus = async () => {
  statusLoading.value = true
  try {
    rebuildStatus.value = await api.getIndexRebuildStatus()
  } catch (e) {
    error.value = e?.message || '获取重建状态失败'
  } finally {
    statusLoading.value = false
  }
}

const refreshLexicons = async () => {
  const data = await api.listLexicons({
    q: lexiconQuery.value.trim() || undefined,
    enabled: resolveEnabledFilter(lexiconEnabledFilter.value),
    limit: 100,
    offset: 0,
  })
  lexicons.value = data?.items || []
  lexiconTotal.value = Number(data?.total || 0)
}

const refreshSynonyms = async () => {
  const data = await api.listSynonyms({
    q: synonymQuery.value.trim() || undefined,
    enabled: resolveEnabledFilter(synonymEnabledFilter.value),
    limit: 100,
    offset: 0,
  })
  synonyms.value = data?.items || []
  synonymTotal.value = Number(data?.total || 0)
}

const refreshAll = async () => {
  loading.value = true
  clearTips()
  try {
    await Promise.all([refreshLexicons(), refreshSynonyms(), refreshStatus()])
  } catch (e) {
    error.value = e?.message || '加载失败'
  } finally {
    loading.value = false
  }
}

const submitLexicon = async () => {
  if (busy.value) return
  clearTips()
  const term = String(newLexiconTerm.value ?? '').trim()
  if (!term) {
    error.value = '词条不能为空'
    return
  }
  const freqRaw = String(newLexiconFreq.value ?? '').trim()
  let freq
  if (freqRaw) {
    freq = Number(freqRaw)
    if (!Number.isInteger(freq) || freq <= 0) {
      error.value = '词频必须是正整数'
      return
    }
  }

  busy.value = true
  try {
    const payload = {
      term,
      enabled: newLexiconEnabled.value,
    }
    if (freq) payload.freq = freq
    const tag = String(newLexiconTag.value ?? '').trim()
    if (tag) payload.tag = tag
    await api.createLexicon(payload)
    newLexiconTerm.value = ''
    newLexiconFreq.value = ''
    newLexiconTag.value = ''
    newLexiconEnabled.value = true
    success.value = '词条已保存（待发布后生效）'
    markLexiconChanged()
    await refreshLexicons()
  } catch (e) {
    error.value = e?.message || '新增词条失败'
  } finally {
    busy.value = false
  }
}

const editLexicon = (item) => {
  clearTips()
  editDialog.value = { type: 'lexicon', item }
  editForm.value = { term: item.term || '', freq: item.freq ?? '', tag: item.tag || '', enabled: Boolean(item.enabled) }
}

const editSynonym = (item) => {
  clearTips()
  editDialog.value = { type: 'synonym', item }
  editForm.value = { term: item.term || '', synonym: item.synonym || '', weight: item.weight ?? 1, bidirectional: Boolean(item.bidirectional), enabled: Boolean(item.enabled) }
}

const closeEditDialog = () => { if (!busy.value) editDialog.value = null }

const submitEdit = async () => {
  if (!editDialog.value || busy.value) return
  const form = editForm.value
  if (!String(form.term || '').trim()) { error.value = '原词不能为空'; return }
  busy.value = true
  try {
    if (editDialog.value.type === 'lexicon') {
      const payload = { term: String(form.term).trim(), enabled: Boolean(form.enabled), tag: String(form.tag || '').trim() }
      if (String(form.freq || '').trim()) {
        const freq = Number(form.freq)
        if (!Number.isInteger(freq) || freq <= 0) { error.value = '词频必须是正整数'; busy.value = false; return }
        payload.freq = freq
      }
      await api.updateLexicon(editDialog.value.item.id, payload)
      success.value = '词条已更新（待发布后生效）'
      markLexiconChanged()
      await refreshLexicons()
    } else {
      const synonym = String(form.synonym || '').trim()
      const weight = Number(form.weight)
      if (!synonym) { error.value = '同义词不能为空'; busy.value = false; return }
      if (!Number.isFinite(weight) || weight <= 0) { error.value = '权重必须大于 0'; busy.value = false; return }
      await api.updateSynonym(editDialog.value.item.id, { term: String(form.term).trim(), synonym, weight, bidirectional: Boolean(form.bidirectional), enabled: Boolean(form.enabled) })
      success.value = '同义词已更新'
      await refreshSynonyms()
    }
    editDialog.value = null
  } catch (e) {
    error.value = e?.message || '更新失败'
  } finally {
    busy.value = false
  }
}
const toggleLexicon = async (item) => {
  if (busy.value) return
  clearTips()
  busy.value = true
  try {
    await api.toggleLexiconEnabled(item.id, !item.enabled)
    success.value = '词条状态已更新（待发布后生效）'
    markLexiconChanged()
    await refreshLexicons()
  } catch (e) {
    error.value = e?.message || '更新状态失败'
  } finally {
    busy.value = false
  }
}

const deleteLexicon = async (item) => {
  clearTips()
  busy.value = true
  try {
    await api.deleteLexicon(item.id)
    success.value = '词条已删除（待发布后生效）'
    markLexiconChanged()
    await refreshLexicons()
  } catch (e) {
    error.value = e?.message || '删除词条失败'
  } finally {
    busy.value = false
  }
}

const submitSynonym = async () => {
  if (busy.value) return
  clearTips()
  const term = newSynonymTerm.value.trim()
  const synonym = newSynonymValue.value.trim()
  if (!term || !synonym) {
    error.value = '原词和同义词不能为空'
    return
  }
  const weight = Number(newSynonymWeight.value)
  if (!Number.isFinite(weight) || weight <= 0) {
    error.value = '权重必须大于 0'
    return
  }

  busy.value = true
  try {
    await api.createSynonym({
      term,
      synonym,
      weight,
      bidirectional: newSynonymBidirectional.value,
      enabled: newSynonymEnabled.value,
    })
    newSynonymTerm.value = ''
    newSynonymValue.value = ''
    newSynonymWeight.value = '1'
    newSynonymBidirectional.value = true
    newSynonymEnabled.value = true
    success.value = '同义词已保存（查询立即生效）'
    await refreshSynonyms()
  } catch (e) {
    error.value = e?.message || '新增同义词失败'
  } finally {
    busy.value = false
  }
}


const toggleSynonym = async (item) => {
  if (busy.value) return
  clearTips()
  busy.value = true
  try {
    await api.toggleSynonymEnabled(item.id, !item.enabled)
    success.value = '同义词状态已更新'
    await refreshSynonyms()
  } catch (e) {
    error.value = e?.message || '更新状态失败'
  } finally {
    busy.value = false
  }
}

const deleteSynonym = async (item) => {
  clearTips()
  busy.value = true
  try {
    await api.deleteSynonym(item.id)
    success.value = '同义词已删除'
    await refreshSynonyms()
  } catch (e) {
    error.value = e?.message || '删除同义词失败'
  } finally {
    busy.value = false
  }
}

const requestDelete = (type, item) => {
  confirmDialog.value = {
    title: type === 'lexicon' ? '删除词条' : '删除同义词',
    message: type === 'lexicon' ? `确定删除“${item.term}”吗？` : `确定删除“${item.term} → ${item.synonym}”吗？`,
    action: () => type === 'lexicon' ? deleteLexicon(item) : deleteSynonym(item),
  }
}
const confirmAction = async () => {
  const action = confirmDialog.value?.action
  confirmDialog.value = null
  await action?.()
}

const batchSetEnabled = async (type, enabled) => {
  const ids = type === 'lexicon' ? selectedLexicons.value : selectedSynonyms.value
  if (!ids.length || busy.value) return
  busy.value = true
  batchProgress.value = { done: 0, total: 0, action: enabled ? '启用' : '停用' }
  clearTips()
  try {
    const items = (type === 'lexicon' ? lexicons.value : synonyms.value).filter((item) => ids.includes(item.id) && item.enabled !== enabled)
    batchProgress.value.total = items.length
    const outcomes = await Promise.allSettled(items.map((item) => {
      const task = type === 'lexicon' ? api.toggleLexiconEnabled(item.id, enabled) : api.toggleSynonymEnabled(item.id, enabled)
      return task.finally(() => { batchProgress.value.done++ })
    }))
    const failed = outcomes.filter((result) => result.status === 'rejected').length
    const succeeded = items.length - failed
    if (type === 'lexicon' && succeeded) { pendingChanges.value += succeeded; localStorage.setItem('htknow_lexicon_pending', String(pendingChanges.value)) }
    selectedLexicons.value = []
    selectedSynonyms.value = []
    await (type === 'lexicon' ? refreshLexicons() : refreshSynonyms())
    success.value = `已${enabled ? '启用' : '停用'} ${succeeded} 条记录${failed ? `，${failed} 条失败` : ''}`
  } catch (e) { error.value = e?.message || '批量操作失败' }
  finally { busy.value = false; setTimeout(() => { batchProgress.value = null }, 1200) }
}
const requestBatchDelete = (type) => {
  const count = type === 'lexicon' ? selectedLexicons.value.length : selectedSynonyms.value.length
  if (!count) return
  confirmDialog.value = { title: `批量删除${type === 'lexicon' ? '词条' : '同义词'}`, message: `确定删除选中的 ${count} 条记录吗？`, action: () => batchDelete(type) }
}
const batchDelete = async (type) => {
  const ids = [...(type === 'lexicon' ? selectedLexicons.value : selectedSynonyms.value)]
  if (!ids.length || busy.value) return
  busy.value = true
  batchProgress.value = { done: 0, total: ids.length, action: '删除' }
  clearTips()
  try {
    const outcomes = await Promise.allSettled(ids.map((id) => {
      const task = type === 'lexicon' ? api.deleteLexicon(id) : api.deleteSynonym(id)
      return task.finally(() => { batchProgress.value.done++ })
    }))
    const failed = outcomes.filter((result) => result.status === 'rejected').length
    const succeeded = ids.length - failed
    if (type === 'lexicon' && succeeded) { pendingChanges.value += succeeded; localStorage.setItem('htknow_lexicon_pending', String(pendingChanges.value)) }
    selectedLexicons.value = []
    selectedSynonyms.value = []
    await (type === 'lexicon' ? refreshLexicons() : refreshSynonyms())
    success.value = `已删除 ${succeeded} 条记录${failed ? `，${failed} 条失败` : ''}`
  } catch (e) { error.value = e?.message || '批量删除失败' }
  finally { busy.value = false; setTimeout(() => { batchProgress.value = null }, 1200) }
}

const publishLexicon = async () => {
  if (publishLoading.value) return
  clearTips()
  publishLoading.value = true
  try {
    const resp = await api.publishLexicon()
    success.value = `发布成功，已启动重建任务 #${resp.job_id}`
    pendingChanges.value = 0
    localStorage.removeItem('htknow_lexicon_pending')
    await refreshStatus()
  } catch (e) {
    error.value = e?.message || '发布失败'
  } finally {
    publishLoading.value = false
  }
}

const startStatusPolling = () => {
  if (statusPollTimer) return
  statusPollTimer = setInterval(async () => {
    try {
      await refreshStatus()
    } catch (_) {
      // ignore in background polling
    }
  }, 3000)
}

const stopStatusPolling = () => {
  if (statusPollTimer) {
    clearInterval(statusPollTimer)
    statusPollTimer = null
  }
}

onMounted(async () => {
  await refreshAll()
  startStatusPolling()
})

onBeforeUnmount(() => {
  stopStatusPolling()
})
</script>

<template>
  <div class="space-y-6">
    <div class="bg-white rounded-2xl border border-slate-200 p-6">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h3 class="text-xl font-semibold text-slate-800">词表发布与索引重建</h3>
          <p class="text-slate-500 text-sm mt-1">词表改动在“发布”后生效，发布会暂停解析并重建索引。</p>
          <p v-if="pendingChanges" class="pending-changes">{{ pendingChanges }} 项修改等待发布</p>
        </div>
        <div class="flex items-center gap-2">
          <button
            @click="refreshAll"
            :disabled="loading || busy || publishLoading"
            class="px-4 py-2 rounded-lg border border-slate-200 text-slate-600 hover:text-slate-900 hover:border-slate-300 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            刷新
          </button>
          <button
            @click="publishLexicon"
            :disabled="publishLoading || isRebuilding || !pendingChanges"
            class="px-4 py-2 rounded-lg bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {{ publishLoading ? '发布中...' : '发布并重建索引' }}
          </button>
        </div>
      </div>

      <div class="mt-4 grid grid-cols-1 md:grid-cols-3 gap-3">
        <div class="rounded-xl border border-slate-200 p-3">
          <div class="text-xs text-slate-400 mb-1">状态</div>
          <span :class="['inline-flex px-2.5 py-1 rounded-full border text-xs font-medium', statusBadgeClass]">
            {{ rebuildStatus.status || 'idle' }}
          </span>
        </div>
        <div class="rounded-xl border border-slate-200 p-3">
          <div class="text-xs text-slate-400 mb-1">阶段</div>
          <div class="text-sm text-slate-700">{{ rebuildStatus.phase || '-' }}</div>
        </div>
        <div class="rounded-xl border border-slate-200 p-3">
          <div class="text-xs text-slate-400 mb-1">任务 ID</div>
          <div class="text-sm text-slate-700">{{ rebuildStatus.job_id ?? '-' }}</div>
        </div>
      </div>

      <div class="mt-4">
        <div class="flex justify-between text-xs text-slate-500 mb-1">
          <span>进度 {{ progressText }}</span>
          <span>{{ progressPercent.toFixed(1) }}%</span>
        </div>
        <div class="w-full h-2 bg-slate-100 rounded-full overflow-hidden">
          <div class="h-full bg-linear-to-r from-blue-500 to-indigo-600" :style="{ width: `${progressPercent}%` }" />
        </div>
      </div>

      <div class="mt-3 grid grid-cols-2 md:grid-cols-4 gap-2 text-xs">
        <div class="rounded-lg bg-slate-50 px-3 py-2">
          <div class="text-slate-400">已运行</div>
          <div class="text-slate-700">{{ formatSecs(rebuildStatus.elapsed_secs) }}</div>
        </div>
        <div class="rounded-lg bg-slate-50 px-3 py-2">
          <div class="text-slate-400">ETA</div>
          <div class="text-slate-700">{{ formatSecs(rebuildStatus.eta_secs) }}</div>
        </div>
        <div class="rounded-lg bg-slate-50 px-3 py-2">
          <div class="text-slate-400">开始时间</div>
          <div class="text-slate-700">{{ formatTime(rebuildStatus.started_at) }}</div>
        </div>
        <div class="rounded-lg bg-slate-50 px-3 py-2">
          <div class="text-slate-400">最近更新</div>
          <div class="text-slate-700">{{ formatTime(rebuildStatus.updated_at) }}</div>
        </div>
      </div>

      <p v-if="statusLoading" class="mt-3 text-xs text-slate-500">状态刷新中...</p>
      <p v-if="rebuildStatus.error" class="mt-3 text-sm text-red-600 bg-red-50 border border-red-100 rounded-lg p-3">
        {{ rebuildStatus.error }}
      </p>
      <p v-if="success" class="mt-3 text-sm text-emerald-700 bg-emerald-50 border border-emerald-100 rounded-lg p-3">
        {{ success }}
      </p>
      <p v-if="error" class="mt-3 text-sm text-red-700 bg-red-50 border border-red-100 rounded-lg p-3">
        {{ error }}
      </p>
    </div>

    <div class="grid grid-cols-1 xl:grid-cols-2 gap-6">
      <section class="bg-white rounded-2xl border border-slate-200 p-6 space-y-4">
        <div class="flex items-center justify-between">
          <h3 class="text-lg font-semibold text-slate-800">词表管理</h3>
          <span class="text-xs text-slate-500">共 {{ lexiconTotal }} 条</span>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-4 gap-2">
          <input
            v-model="lexiconQuery"
            type="text"
            class="md:col-span-2 px-3 py-2 border border-slate-200 rounded-lg text-sm"
            placeholder="搜索 term"
          />
          <select v-model="lexiconEnabledFilter" class="px-3 py-2 border border-slate-200 rounded-lg text-sm">
            <option value="all">全部</option>
            <option value="enabled">仅启用</option>
            <option value="disabled">仅停用</option>
          </select>
          <button
            @click="refreshLexicons"
            class="px-3 py-2 rounded-lg border border-slate-200 text-slate-600 hover:text-slate-900 text-sm"
          >
            筛选
          </button>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-5 gap-2">
          <input v-model="newLexiconTerm" type="text" class="px-3 py-2 border border-slate-200 rounded-lg text-sm" placeholder="term" />
          <input v-model="newLexiconFreq" type="number" min="1" class="px-3 py-2 border border-slate-200 rounded-lg text-sm" placeholder="freq" />
          <input v-model="newLexiconTag" type="text" class="px-3 py-2 border border-slate-200 rounded-lg text-sm" placeholder="tag" />
          <label class="px-3 py-2 border border-slate-200 rounded-lg text-sm flex items-center gap-2">
            <input v-model="newLexiconEnabled" type="checkbox" />
            启用
          </label>
          <button
            @click="submitLexicon"
            :disabled="busy"
            class="px-3 py-2 rounded-lg bg-slate-900 text-white text-sm hover:bg-slate-800 disabled:opacity-50"
          >
            新增
          </button>
        </div>

        <div v-if="selectedLexicons.length" class="batch-toolbar"><span>已选择 {{ selectedLexicons.length }} 项</span><span v-if="batchProgress">{{ batchProgress.action }}中 {{ batchProgress.done }} / {{ batchProgress.total }}</span><button :disabled="busy" @click="batchSetEnabled('lexicon', true)">启用</button><button :disabled="busy" @click="batchSetEnabled('lexicon', false)">停用</button><button class="text-red-600" :disabled="busy" @click="requestBatchDelete('lexicon')">删除</button><button :disabled="busy" @click="selectedLexicons = []">取消选择</button></div>

        <div class="dictionary-table border border-slate-200 rounded-xl overflow-auto">
          <table class="w-full text-sm">
            <thead class="bg-slate-50 text-slate-500">
              <tr>
                <th class="px-3 py-2"><input type="checkbox" aria-label="选择全部词条" :checked="lexicons.length > 0 && selectedLexicons.length === lexicons.length" @change="selectedLexicons = $event.target.checked ? lexicons.map(item => item.id) : []" /></th>
                <th class="text-left px-3 py-2">term</th>
                <th class="text-left px-3 py-2">freq</th>
                <th class="text-left px-3 py-2">tag</th>
                <th class="text-left px-3 py-2">状态</th>
                <th class="text-right px-3 py-2">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-if="lexicons.length === 0">
                <td colspan="6" class="px-3 py-6 text-center text-slate-400">暂无词表数据</td>
              </tr>
              <tr v-for="item in lexicons" :key="item.id" class="border-t border-slate-100">
                <td class="px-3 py-2"><input v-model="selectedLexicons" type="checkbox" :value="item.id" :aria-label="`选择词条 ${item.term}`" /></td>
                <td class="px-3 py-2 text-slate-800 break-all">{{ item.term }}</td>
                <td class="px-3 py-2 text-slate-600">{{ item.freq ?? '-' }}</td>
                <td class="px-3 py-2 text-slate-600">{{ item.tag || '-' }}</td>
                <td class="px-3 py-2">
                  <span
                    :class="[
                      'px-2 py-0.5 rounded-full text-xs border',
                      item.enabled ? 'bg-emerald-50 text-emerald-700 border-emerald-200' : 'bg-slate-50 text-slate-500 border-slate-200'
                    ]"
                  >
                    {{ item.enabled ? '启用' : '停用' }}
                  </span>
                </td>
                <td class="px-3 py-2">
                  <div class="flex justify-end gap-1">
                    <button @click="editLexicon(item)" class="px-2 py-1 text-xs border border-slate-200 rounded hover:bg-slate-50">编辑</button>
                    <button @click="toggleLexicon(item)" class="px-2 py-1 text-xs border border-slate-200 rounded hover:bg-slate-50">
                      {{ item.enabled ? '停用' : '启用' }}
                    </button>
                    <button @click="requestDelete('lexicon', item)" class="px-2 py-1 text-xs border border-red-200 text-red-600 rounded hover:bg-red-50">删除</button>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <section class="bg-white rounded-2xl border border-slate-200 p-6 space-y-4">
        <div class="flex items-center justify-between">
          <h3 class="text-lg font-semibold text-slate-800">同义词管理</h3>
          <span class="text-xs text-slate-500">共 {{ synonymTotal }} 条</span>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-4 gap-2">
          <input
            v-model="synonymQuery"
            type="text"
            class="md:col-span-2 px-3 py-2 border border-slate-200 rounded-lg text-sm"
            placeholder="搜索 term/synonym"
          />
          <select v-model="synonymEnabledFilter" class="px-3 py-2 border border-slate-200 rounded-lg text-sm">
            <option value="all">全部</option>
            <option value="enabled">仅启用</option>
            <option value="disabled">仅停用</option>
          </select>
          <button
            @click="refreshSynonyms"
            class="px-3 py-2 rounded-lg border border-slate-200 text-slate-600 hover:text-slate-900 text-sm"
          >
            筛选
          </button>
        </div>

        <div class="grid grid-cols-1 md:grid-cols-6 gap-2">
          <input v-model="newSynonymTerm" type="text" class="px-3 py-2 border border-slate-200 rounded-lg text-sm" placeholder="term" />
          <input v-model="newSynonymValue" type="text" class="px-3 py-2 border border-slate-200 rounded-lg text-sm" placeholder="synonym" />
          <input v-model="newSynonymWeight" type="number" min="0.01" step="0.01" class="px-3 py-2 border border-slate-200 rounded-lg text-sm" placeholder="weight" />
          <label class="px-3 py-2 border border-slate-200 rounded-lg text-sm flex items-center gap-2">
            <input v-model="newSynonymBidirectional" type="checkbox" />
            双向
          </label>
          <label class="px-3 py-2 border border-slate-200 rounded-lg text-sm flex items-center gap-2">
            <input v-model="newSynonymEnabled" type="checkbox" />
            启用
          </label>
          <button
            @click="submitSynonym"
            :disabled="busy"
            class="px-3 py-2 rounded-lg bg-slate-900 text-white text-sm hover:bg-slate-800 disabled:opacity-50"
          >
            新增
          </button>
        </div>

        <div v-if="selectedSynonyms.length" class="batch-toolbar"><span>已选择 {{ selectedSynonyms.length }} 项</span><span v-if="batchProgress">{{ batchProgress.action }}中 {{ batchProgress.done }} / {{ batchProgress.total }}</span><button :disabled="busy" @click="batchSetEnabled('synonym', true)">启用</button><button :disabled="busy" @click="batchSetEnabled('synonym', false)">停用</button><button class="text-red-600" :disabled="busy" @click="requestBatchDelete('synonym')">删除</button><button :disabled="busy" @click="selectedSynonyms = []">取消选择</button></div>

        <div class="dictionary-table border border-slate-200 rounded-xl overflow-auto">
          <table class="w-full text-sm">
            <thead class="bg-slate-50 text-slate-500">
              <tr>
                <th class="px-3 py-2"><input type="checkbox" aria-label="选择全部同义词" :checked="synonyms.length > 0 && selectedSynonyms.length === synonyms.length" @change="selectedSynonyms = $event.target.checked ? synonyms.map(item => item.id) : []" /></th>
                <th class="text-left px-3 py-2">term</th>
                <th class="text-left px-3 py-2">synonym</th>
                <th class="text-left px-3 py-2">权重</th>
                <th class="text-left px-3 py-2">双向</th>
                <th class="text-left px-3 py-2">状态</th>
                <th class="text-right px-3 py-2">操作</th>
              </tr>
            </thead>
            <tbody>
              <tr v-if="synonyms.length === 0">
                <td colspan="7" class="px-3 py-6 text-center text-slate-400">暂无同义词数据</td>
              </tr>
              <tr v-for="item in synonyms" :key="item.id" class="border-t border-slate-100">
                <td class="px-3 py-2"><input v-model="selectedSynonyms" type="checkbox" :value="item.id" :aria-label="`选择同义词 ${item.term}`" /></td>
                <td class="px-3 py-2 text-slate-800 break-all">{{ item.term }}</td>
                <td class="px-3 py-2 text-slate-700 break-all">{{ item.synonym }}</td>
                <td class="px-3 py-2 text-slate-600">{{ item.weight }}</td>
                <td class="px-3 py-2 text-slate-600">{{ item.bidirectional ? '是' : '否' }}</td>
                <td class="px-3 py-2">
                  <span
                    :class="[
                      'px-2 py-0.5 rounded-full text-xs border',
                      item.enabled ? 'bg-emerald-50 text-emerald-700 border-emerald-200' : 'bg-slate-50 text-slate-500 border-slate-200'
                    ]"
                  >
                    {{ item.enabled ? '启用' : '停用' }}
                  </span>
                </td>
                <td class="px-3 py-2">
                  <div class="flex justify-end gap-1">
                    <button @click="editSynonym(item)" class="px-2 py-1 text-xs border border-slate-200 rounded hover:bg-slate-50">编辑</button>
                    <button @click="toggleSynonym(item)" class="px-2 py-1 text-xs border border-slate-200 rounded hover:bg-slate-50">
                      {{ item.enabled ? '停用' : '启用' }}
                    </button>
                    <button @click="requestDelete('synonym', item)" class="px-2 py-1 text-xs border border-red-200 text-red-600 rounded hover:bg-red-50">删除</button>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>
    </div>
  </div>
  <Teleport to="body">
    <div v-if="editDialog" class="upload-overlay" @click.self="closeEditDialog" @keydown.esc="closeEditDialog">
      <section v-dialog class="upload-dialog max-w-lg" role="dialog" aria-modal="true" aria-label="编辑词条">
        <div class="panel-heading"><div><h2>编辑{{ editDialog.type === 'lexicon' ? '词条' : '同义词' }}</h2><p>修改后保存即可，发布索引后生效。</p></div><button class="plain-button" @click="closeEditDialog" :disabled="busy" aria-label="关闭">✕</button></div>
        <form class="p-6 space-y-4" @submit.prevent="submitEdit">
          <label class="block text-sm text-slate-600">原词<input v-model="editForm.term" class="mt-1 w-full px-3 py-2 border border-slate-200 rounded-lg" required /></label>
          <label v-if="editDialog.type === 'synonym'" class="block text-sm text-slate-600">同义词<input v-model="editForm.synonym" class="mt-1 w-full px-3 py-2 border border-slate-200 rounded-lg" required /></label>
          <div v-if="editDialog.type === 'lexicon'" class="grid grid-cols-2 gap-3"><label class="text-sm text-slate-600">词频<input v-model="editForm.freq" type="number" min="1" class="mt-1 w-full px-3 py-2 border border-slate-200 rounded-lg" /></label><label class="text-sm text-slate-600">词性<input v-model="editForm.tag" class="mt-1 w-full px-3 py-2 border border-slate-200 rounded-lg" /></label></div>
          <label v-else class="block text-sm text-slate-600">权重<input v-model="editForm.weight" type="number" min="0.01" step="0.01" class="mt-1 w-full px-3 py-2 border border-slate-200 rounded-lg" /></label>
          <div class="flex flex-wrap gap-4 text-sm text-slate-600"><label><input v-model="editForm.enabled" type="checkbox" class="mr-2" />启用</label><label v-if="editDialog.type === 'synonym'"><input v-model="editForm.bidirectional" type="checkbox" class="mr-2" />双向</label></div>
          <div class="dialog-actions"><button type="button" class="secondary-button" @click="closeEditDialog">取消</button><button type="submit" class="primary-button" :disabled="busy">保存</button></div>
        </form>
      </section>
    </div>
  </Teleport>
  <Teleport to="body">
    <div v-if="confirmDialog" class="upload-overlay" @click.self="confirmDialog = null" @keydown.esc="confirmDialog = null">
      <section v-dialog class="confirm-dialog" role="alertdialog" aria-modal="true" aria-label="确认删除">
        <h2>{{ confirmDialog.title }}</h2><p>{{ confirmDialog.message }} 此操作无法撤销。</p>
        <div class="dialog-actions"><button class="secondary-button" @click="confirmDialog = null">取消</button><button class="primary-button danger-button" @click="confirmAction">删除</button></div>
      </section>
    </div>
  </Teleport>
</template>
