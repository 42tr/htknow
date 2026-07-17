<script setup>
import { ref, onMounted, computed, nextTick } from 'vue'
import { api } from '../api'
import FileCard from './FileCard.vue'
import CreateKnowledgeBase from './CreateKnowledgeBase.vue'
import FileStatusSummary from './FileStatusSummary.vue'
import ExportRecordPanel from './ExportRecordPanel.vue'
import KbPermissionModal from './KbPermissionModal.vue'
import Pagination from './Pagination.vue'
import { setCurrentKb } from '../store'

// Reactive state for the current view
const currentKb = ref(null) // The KB we are currently inside, null for root
const childrenKbs = ref([])
const files = ref([])
const breadcrumbs = ref([])
const loading = ref(true)
const error = ref('')
const reparseLoading = ref(false)
const reparseFailedLoading = ref(false)
const currentKbReparseLoading = ref(false)
const childKbReparseLoading = ref({})
const priorityDrafts = ref({})
const prioritySaving = ref({})
const locatedFileId = ref(null)

// Pagination / filter state for KB file list
const currentPage = ref(1)
const pageSize = ref(10)
const totalFiles = ref(0)
const fileFilterName = ref('')
const fileFilterTag = ref('')

// Permission modal state
const showPermissionModal = ref(false)
const permissionModalKb = ref(null)

const openPermissionModal = (kb) => {
  permissionModalKb.value = kb
  showPermissionModal.value = true
}

// Export state
const selectedKbs = ref(new Map())
const exportLoading = ref(false)
const exportIncludeChildren = ref(false)
const exportRecords = ref([])
const EXPORT_RECORDS_KEY = 'htknow_export_records'

const loadExportRecords = () => {
  try {
    const raw = localStorage.getItem(EXPORT_RECORDS_KEY)
    if (raw) exportRecords.value = JSON.parse(raw)
  } catch {
    exportRecords.value = []
  }
}

const saveExportRecords = () => {
  localStorage.setItem(EXPORT_RECORDS_KEY, JSON.stringify(exportRecords.value))
}

const addExportRecord = (result) => {
  const manifest = result.manifest || {}
  const record = {
    id: Date.now(),
    timestamp: new Date().toISOString(),
    exportPath: result.export_path || '',
    kbNames: manifest.kb_names || [],
    kbCount: manifest.kb_ids?.length || 0,
    kb_ids: manifest.kb_ids || [],
    fileCount: manifest.file_count || 0,
    sliceCount: manifest.slice_count || 0,
    tantivyDocCount: manifest.tantivy_doc_count || 0,
    lancedbRowCount: manifest.lancedb_row_count || 0,
  }
  exportRecords.value.unshift(record)
  if (exportRecords.value.length > 50) {
    exportRecords.value = exportRecords.value.slice(0, 50)
  }
  saveExportRecords()
}

const clearExportRecords = () => {
  exportRecords.value = []
  localStorage.removeItem(EXPORT_RECORDS_KEY)
}

const selectedKbIds = computed(() => Array.from(selectedKbs.value.keys()))
const selectedKbCount = computed(() => selectedKbs.value.size)
const hasSelectedKbs = computed(() => selectedKbCount.value > 0)
const selectedKbNames = computed(() => Array.from(selectedKbs.value.values()).map(kb => kb.name))
const selectedKbPreview = computed(() => {
  if (selectedKbNames.value.length === 0) return ''
  if (selectedKbNames.value.length <= 3) return selectedKbNames.value.join('、')
  return `${selectedKbNames.value.slice(0, 3).join('、')} 等 ${selectedKbNames.value.length} 个`
})

const toggleKbSelection = (kb) => {
  const next = new Map(selectedKbs.value)
  if (next.has(kb.id)) {
    next.delete(kb.id)
  } else {
    next.set(kb.id, { id: kb.id, name: kb.name })
  }
  selectedKbs.value = next
}

const selectAllKbs = () => {
  const currentPageIds = childrenKbs.value.map(kb => kb.id)
  const allSelected = currentPageIds.length > 0 && currentPageIds.every(id => selectedKbs.value.has(id))
  const next = new Map(selectedKbs.value)

  if (allSelected) {
    currentPageIds.forEach(id => next.delete(id))
  } else {
    childrenKbs.value.forEach(kb => next.set(kb.id, { id: kb.id, name: kb.name }))
  }
  selectedKbs.value = next
}

const clearSelectedKbs = () => {
  selectedKbs.value = new Map()
}

const handleExport = async () => {
  if (selectedKbCount.value === 0) return
  const ids = selectedKbIds.value
  const names = selectedKbNames.value
  const label = names.length <= 2 ? names.join('、') : `${names[0]} 等 ${names.length} 个`
  if (!confirm(`确定要导出「${label}」${exportIncludeChildren.value ? '（含子知识库）' : ''}吗？`)) return

  exportLoading.value = true
  try {
    const result = await api.exportKnowledgeBases(ids, exportIncludeChildren.value)
    addExportRecord(result)
    alert(`导出成功！\n路径：${result.export_path}`)
    clearSelectedKbs()
  } catch (e) {
    alert('导出失败：' + e.message)
  } finally {
    exportLoading.value = false
  }
}

const createEmptyStats = () => ({
  total: 0,
  pending: 0,
  processing: 0,
  completed: 0,
  skipped: 0,
  failed: 0,
  unknown: 0,
  processing_files: [],
  failed_files: [],
})
const stats = ref(createEmptyStats())
const statsLoading = ref(true)
const statsError = ref('')

const statsSubtitle = computed(() => {
  if (currentKb.value && currentKb.value.id !== null) {
    return `覆盖 ${currentKb.value.name} 及其子知识库`
  }
  return '覆盖所有知识库（含未分配文件）'
})

const getCurrentKbId = () => {
  return currentKb.value?.id ?? null
}

const fetchStats = async (kbId) => {
  statsLoading.value = true
  statsError.value = ''
  try {
    const params = {}
    if (kbId === null || kbId === undefined) {
      // 全局统计，后台默认包含未分配文件
    } else {
      params.kbId = kbId
      params.includeDescendants = true
    }
    stats.value = await api.getFileStats(params)
  } catch (e) {
    statsError.value = e?.message || '加载统计失败'
  } finally {
    statsLoading.value = false
  }
}

const loadKbContent = async (kbId) => {
  const targetId = kbId ?? null
  loading.value = true
  error.value = ''
  try {
    let newCurrentKb;
    if (targetId === null) {
      // Root view: fetch top-level KBs and unassigned files
      const [topLevelKbs, unassignedFiles] = await Promise.all([
        api.getKnowledgeBases(null),
        api.getFiles(null, null, { page: currentPage.value, size: pageSize.value })
      ]);
      childrenKbs.value = topLevelKbs
      files.value = unassignedFiles.items || []
      totalFiles.value = unassignedFiles.total || 0
      newCurrentKb = { id: null, name: '所有知识库', kb_type: null }
      breadcrumbs.value = []
    } else {
      // Inside a specific KB
      const [data, filesData] = await Promise.all([
        api.getKnowledgeBase(targetId),
        api.getKnowledgeBaseFiles(targetId, {
          page: currentPage.value,
          size: pageSize.value,
          filename: fileFilterName.value || undefined,
          tag: fileFilterTag.value || undefined,
        })
      ])
      childrenKbs.value = data.children_kbs || []
      files.value = filesData.items || []
      totalFiles.value = filesData.total || 0
      newCurrentKb = { id: data.id, name: data.name, description: data.description, kb_type: data.kb_type }
      breadcrumbs.value = data.path || []
    }
    const nextPriorityDrafts = {}
    for (const kb of childrenKbs.value) {
      nextPriorityDrafts[kb.id] = Number.isInteger(kb.parse_priority) ? kb.parse_priority : 50
    }
    priorityDrafts.value = nextPriorityDrafts
    currentKb.value = newCurrentKb;
    setCurrentKb(newCurrentKb); // Update global store
    await fetchStats(targetId)
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

// --- Navigation ---
const navigateToKb = (kbId) => {
  currentPage.value = 1
  fileFilterName.value = ''
  fileFilterTag.value = ''
  loadKbContent(kbId)
}

const applyFileFilters = () => {
  currentPage.value = 1
  loadKbContent(getCurrentKbId())
}

const handleLocateFile = async (file) => {
  if (!file?.id) return

  locatedFileId.value = null
  currentPage.value = 1
  fileFilterName.value = ''
  fileFilterTag.value = ''
  await loadKbContent(file.kb_id ?? null)
  await nextTick()

  const target = document.getElementById(`file-card-${file.id}`)
  if (!target) {
    alert('文件所在知识库已打开，但未找到该文件，文件可能已被移动或删除')
    return
  }

  locatedFileId.value = file.id
  target.scrollIntoView({ behavior: 'smooth', block: 'center' })
  window.setTimeout(() => {
    if (locatedFileId.value === file.id) locatedFileId.value = null
  }, 3000)
}

// --- Event Handlers ---
const handleKbCreated = () => {
  loadKbContent(getCurrentKbId())
}

const handleDeleteKb = async (e, kbId) => {
  e.stopPropagation()
  if (!confirm('确定要删除这个知识库及其所有内容吗？此操作不可逆！')) return

  try {
    await api.deleteKnowledgeBase(kbId)
    await loadKbContent(getCurrentKbId()) // Refresh current view
  } catch (e) {
    alert('删除失败：' + e.message)
  }
}

const handleFileAction = () => {
  loadKbContent(getCurrentKbId())
}

const handleTogglePublic = async (e, kbId, currentPublic) => {
  e.stopPropagation()
  const newPublic = !currentPublic
  if (!confirm(`确定要将知识库设置为${newPublic ? '公开' : '私有'}吗？`)) return

  try {
    await api.updateKnowledgeBase(kbId, { is_public: newPublic })
    await loadKbContent(getCurrentKbId())
  } catch (e) {
    alert('更新失败：' + e.message)
  }
}

const handleReparse = async () => {
  if (!confirm('确定要重新解析所有知识库及未分配文件吗？')) return

  reparseLoading.value = true
  try {
    const result = await api.reparseKnowledgeBases()
    const count = result?.file_count ?? 0
    alert(`已提交重新解析任务，共 ${count} 个文件`)
    await loadKbContent(getCurrentKbId())
  } catch (e) {
    alert('重新解析失败：' + e.message)
  } finally {
    reparseLoading.value = false
  }
}

const handleReparseFailedFiles = async () => {
  const failedCount = stats.value?.failed ?? 0
  if (failedCount <= 0) {
    alert('当前范围内没有处理失败的文件')
    return
  }

  const isRootScope = !currentKb.value || currentKb.value.id === null
  const scopeLabel = isRootScope
    ? '所有知识库及未分配文件中的失败文件'
    : `「${currentKb.value.name || '当前知识库'}」及其子知识库中的失败文件`

  if (!confirm(`确定要重新解析${scopeLabel}吗？`)) return

  reparseFailedLoading.value = true
  try {
    const result = isRootScope
      ? await api.reparseFailedFiles()
      : await api.reparseFailedFiles({
          kbId: currentKb.value.id,
          includeDescendants: true,
        })
    const count = result?.file_count ?? 0
    alert(count > 0 ? `已提交 ${count} 个失败文件重新解析` : '当前范围内没有可重新解析的失败文件')
    await loadKbContent(getCurrentKbId())
  } catch (e) {
    alert('重新解析失败文件失败：' + e.message)
  } finally {
    reparseFailedLoading.value = false
  }
}

const submitKbReparse = async (kbId, kbName) => {
  const result = await api.reparseKnowledgeBase(kbId)
  const kbCount = result?.kb_count ?? 0
  const fileCount = result?.file_count ?? 0
  alert(`已提交「${kbName}」重新解析任务（含子知识库），覆盖 ${kbCount} 个知识库、${fileCount} 个文件`)
}

const handleReparseCurrentKb = async () => {
  if (!currentKb.value || currentKb.value.id === null) return
  if (currentKb.value.kb_type === 'storage') {
    alert('存储型知识库不参与解析')
    return
  }
  const kbName = currentKb.value.name || '当前知识库'
  if (!confirm(`确定要重新解析「${kbName}」及其子知识库吗？`)) return

  currentKbReparseLoading.value = true
  try {
    await submitKbReparse(currentKb.value.id, kbName)
    await loadKbContent(getCurrentKbId())
  } catch (e) {
    alert('重新解析失败：' + e.message)
  } finally {
    currentKbReparseLoading.value = false
  }
}

const handleReparseChildKb = async (e, kb) => {
  e.stopPropagation()
  if (!kb || kb.kb_type === 'storage') {
    alert('存储型知识库不参与解析')
    return
  }
  if (childKbReparseLoading.value[kb.id]) return
  if (!confirm(`确定要重新解析「${kb.name}」及其子知识库吗？`)) return

  childKbReparseLoading.value[kb.id] = true
  try {
    await submitKbReparse(kb.id, kb.name)
    await loadKbContent(getCurrentKbId())
  } catch (e) {
    alert('重新解析失败：' + e.message)
  } finally {
    childKbReparseLoading.value[kb.id] = false
  }
}

const handleSaveParsePriority = async (e, kb) => {
  e.stopPropagation()
  if (!kb || kb.kb_type === 'storage') return

  const raw = priorityDrafts.value[kb.id]
  const value = Number(raw)
  if (!Number.isInteger(value) || value < 0 || value > 100) {
    alert('解析优先级必须是 0 到 100 的整数')
    return
  }
  if (value === kb.parse_priority) {
    return
  }

  prioritySaving.value[kb.id] = true
  try {
    await api.updateKnowledgeBase(kb.id, { parse_priority: value })
    kb.parse_priority = value
  } catch (err) {
    alert('保存优先级失败：' + (err?.message || '未知错误'))
    priorityDrafts.value[kb.id] = Number.isInteger(kb.parse_priority) ? kb.parse_priority : 50
  } finally {
    prioritySaving.value[kb.id] = false
  }
}

// Expose refresh method to parent component
defineExpose({
  refresh: () => loadKbContent(getCurrentKbId()),
})

// Initial load
onMounted(() => {
  loadKbContent(null)
  loadExportRecords()
})
</script>

<template>
  <div>
    <!-- Header with Breadcrumbs and Create Button -->
    <div class="flex items-center justify-between gap-4 mb-6">
       <nav class="flex items-center text-sm text-slate-500">
        <span @click="navigateToKb(null)" class="hover:text-blue-500 cursor-pointer">主目录</span>
        <template v-for="crumb in breadcrumbs" :key="crumb.id">
          <span class="mx-2">/</span>
          <span @click="navigateToKb(crumb.id)" class="hover:text-blue-500 cursor-pointer">{{ crumb.name }}</span>
        </template>
        <template v-if="currentKb && currentKb.id !== null">
            <span class="mx-2">/</span>
            <span class="font-semibold text-slate-700">{{ currentKb.name }}</span>
        </template>
      </nav>
      <div class="flex items-center gap-3">
        <button
          v-if="currentKb && currentKb.id === null"
          @click="handleReparse"
          :disabled="reparseLoading"
          title="重新解析所有知识库及未分配文件"
          :class="[
            'px-4 py-2.5 rounded-xl font-medium transition-all duration-200 border flex items-center gap-2',
            reparseLoading
              ? 'bg-slate-100 text-slate-400 border-slate-200 cursor-not-allowed'
              : 'bg-white text-slate-700 border-slate-200 hover:border-blue-300 hover:text-blue-600'
          ]"
        >
          <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v6h6M20 20v-6h-6M5 19a9 9 0 0014-7M19 5a9 9 0 00-14 7" />
          </svg>
          {{ reparseLoading ? '解析中...' : '全部重新解析' }}
        </button>
        <button
          v-if="currentKb && currentKb.id !== null && currentKb.kb_type !== 'storage'"
          @click="handleReparseCurrentKb"
          :disabled="currentKbReparseLoading"
          title="重新解析当前知识库及子知识库"
          :class="[
            'px-4 py-2.5 rounded-xl font-medium transition-all duration-200 border flex items-center gap-2',
            currentKbReparseLoading
              ? 'bg-slate-100 text-slate-400 border-slate-200 cursor-not-allowed'
              : 'bg-white text-slate-700 border-slate-200 hover:border-blue-300 hover:text-blue-600'
          ]"
        >
          <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v6h6M20 20v-6h-6M5 19a9 9 0 0014-7M19 5a9 9 0 00-14 7" />
          </svg>
          {{ currentKbReparseLoading ? '解析中...' : '重新解析当前知识库' }}
        </button>

        <!-- Export Controls -->
        <div v-if="childrenKbs.length > 0 || hasSelectedKbs" class="flex items-center gap-2"
          :class="hasSelectedKbs ? 'opacity-100' : 'opacity-60'"
        >
          <button
            v-if="childrenKbs.length > 0"
            @click="selectAllKbs"
            type="button"
            class="px-3 py-2 text-sm rounded-xl border border-slate-200 bg-white text-slate-600 hover:border-blue-300 hover:text-blue-600 transition-all duration-200"
          >
            {{
              childrenKbs.length > 0 && childrenKbs.every(kb => selectedKbs.has(kb.id))
                ? '取消本层全选'
                : '全选本层'
            }}
          </button>
          <label class="flex items-center gap-1.5 text-sm text-slate-600 cursor-pointer select-none"
            @click.stop
          >
            <input
              v-model="exportIncludeChildren"
              type="checkbox"
              class="w-4 h-4 rounded border-slate-300 text-blue-600 focus:ring-blue-500"
            />
            含子知识库
          </label>
          <button
            @click="handleExport"
            :disabled="!hasSelectedKbs || exportLoading"
            :class="[
              'px-4 py-2.5 rounded-xl font-medium transition-all duration-200 border flex items-center gap-2',
              !hasSelectedKbs || exportLoading
                ? 'bg-slate-100 text-slate-400 border-slate-200 cursor-not-allowed'
                : 'bg-blue-50 text-blue-700 border-blue-200 hover:bg-blue-100 hover:border-blue-300'
            ]"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M9 19l3 3m0 0l3-3m-3 3V10" />
            </svg>
            {{ exportLoading ? '导出中...' : `导出选中 (${selectedKbCount})` }}
          </button>
          <button
            v-if="hasSelectedKbs"
            @click="clearSelectedKbs"
            type="button"
            class="px-3 py-2 text-sm rounded-xl border border-slate-200 bg-white text-slate-500 hover:border-red-300 hover:text-red-600 transition-all duration-200"
          >
            清空已选
          </button>
        </div>

        <CreateKnowledgeBase :parent-id="currentKb?.id" @created="handleKbCreated" />
      </div>
    </div>

    <div
      v-if="hasSelectedKbs"
      class="mb-4 flex items-center justify-between gap-3 rounded-xl border border-blue-200 bg-blue-50 px-4 py-3 text-sm text-blue-800"
    >
      <div class="min-w-0">
        <span class="font-medium">已选 {{ selectedKbCount }} 个知识库：</span>
        <span class="truncate">{{ selectedKbPreview }}</span>
      </div>
      <button
        type="button"
        @click="clearSelectedKbs"
        class="shrink-0 rounded-lg border border-blue-200 bg-white px-3 py-1.5 text-blue-700 hover:border-blue-300"
      >
        清空
      </button>
    </div>

    <FileStatusSummary
      class="mb-4"
      :stats="stats"
      :loading="statsLoading"
      :retry-failed-loading="reparseFailedLoading"
      :error="statsError"
      :title="currentKb && currentKb.id !== null ? '知识库文件状态' : '全局文件状态'"
      :subtitle="statsSubtitle"
      @retry="fetchStats(getCurrentKbId())"
      @reparse-failed="handleReparseFailedFiles"
      @locate-file="handleLocateFile"
    />

    <!-- Loading -->
    <div v-if="loading" class="text-center py-12">
        <p>加载中...</p>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="text-center py-12 text-red-500">
        <p>错误: {{ error }}</p>
        <button @click="loadKbContent(getCurrentKbId())">重试</button>
    </div>

    <!-- Empty State -->
    <div v-else-if="childrenKbs.length === 0 && files.length === 0" class="text-center py-12">
        <div class="w-16 h-16 bg-slate-100 rounded-full flex items-center justify-center mx-auto mb-4">
          <span class="text-3xl">🗂️</span>
        </div>
        <p class="text-slate-500">这个知识库是空的</p>
    </div>

    <!-- Grid for KBs and Files -->
    <div v-else>
      <!-- Child KBs -->
      <div v-if="childrenKbs.length > 0" class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        <div
          v-for="kb in childrenKbs"
          :key="`kb-${kb.id}`"
          @click="navigateToKb(kb.id)"
          class="bg-white rounded-xl p-5 border border-slate-200 hover:border-blue-300 hover:shadow-md transition-all duration-200 group cursor-pointer relative"
        >
          <!-- Selection checkbox -->
          <div class="absolute top-3 left-3 z-10" @click.stop>
            <input
              type="checkbox"
              :checked="selectedKbs.has(kb.id)"
              @change="toggleKbSelection(kb)"
              class="w-5 h-5 rounded border-slate-300 text-blue-600 focus:ring-blue-500 cursor-pointer"
            />
          </div>
          <div class="flex items-start justify-between mb-3 pl-8">
            <div class="w-12 h-12 bg-linear-to-br from-blue-100 to-indigo-100 rounded-xl flex items-center justify-center">
              <span class="text-2xl">📚</span>
            </div>
             <div class="flex items-center gap-1">
                <span
                  v-if="kb.current_user_permission"
                  class="px-2 py-0.5 text-xs rounded-full border"
                  :class="{
                    'bg-purple-50 text-purple-600 border-purple-200': kb.current_user_permission === 'admin',
                    'bg-blue-50 text-blue-600 border-blue-200': kb.current_user_permission === 'editor',
                    'bg-slate-50 text-slate-500 border-slate-200': kb.current_user_permission === 'viewer'
                  }"
                >
                  {{ kb.current_user_permission === 'admin' ? '⚙️ 管理员' : kb.current_user_permission === 'editor' ? '✏️ 可写' : '👁️ 只读' }}
                </span>
                <button
                  v-if="kb.current_user_permission === 'admin'"
                  @click="(e) => { e.stopPropagation(); openPermissionModal(kb) }"
                  class="opacity-0 group-hover:opacity-100 p-2 text-slate-400 hover:text-purple-500 hover:bg-purple-50 rounded-lg transition-all"
                  title="权限管理"
                >
                  <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4.354a4 4 0 110 5.292M15 21H3v-1a6 6 0 0112 0v1zm0 0h6v-1a6 6 0 00-9-5.197M13 7a4 4 0 11-8 0 4 4 0 018 0z" />
                  </svg>
                </button>
                <button
                  v-if="kb.current_user_permission === 'admin'"
                  @click="(e) => handleTogglePublic(e, kb.id, kb.is_public)"
                  :class="[
                    'opacity-0 group-hover:opacity-100 p-2 rounded-lg transition-all',
                    kb.is_public ? 'text-green-500 hover:bg-green-50' : 'text-slate-500 hover:bg-slate-100'
                  ]"
                  :title="kb.is_public ? '设置为私有' : '设置为公开'"
                >
                  <svg v-if="kb.is_public" class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                   <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
                 </svg>
                 <svg v-else class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                   <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3.055 11H5a2 2 0 012 2v1a2 2 0 012 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                 </svg>
               </button>
               <button
                 v-if="kb.current_user_permission === 'editor' || kb.current_user_permission === 'admin'"
                 @click="(e) => handleReparseChildKb(e, kb)"
                 :disabled="kb.kb_type === 'storage' || childKbReparseLoading[kb.id]"
                 class="opacity-0 group-hover:opacity-100 p-2 text-slate-400 hover:text-blue-500 hover:bg-blue-50 rounded-lg transition-all disabled:opacity-40 disabled:cursor-not-allowed"
                 :title="kb.kb_type === 'storage' ? '存储型知识库不参与解析' : (childKbReparseLoading[kb.id] ? '解析中...' : '重新解析该知识库')"
               >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v6h6M20 20v-6h-6M5 19a9 9 0 0014-7M19 5a9 9 0 00-14 7" />
                </svg>
               </button>
               <button
                 v-if="kb.current_user_permission === 'admin'"
                 @click="(e) => handleDeleteKb(e, kb.id)"
                 class="opacity-0 group-hover:opacity-100 p-2 text-slate-400 hover:text-red-500 hover:bg-red-50 rounded-lg transition-all"
                 title="删除"
               >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                   <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                 </svg>
               </button>
             </div>
           </div>
           <h3 class="font-semibold text-slate-800 mb-1 flex items-center gap-2">
             {{ kb.name }}
              <span :class="[
                'px-2 py-0.5 text-xs rounded-full border',
                kb.is_public ? 'bg-green-50 text-green-600 border-green-200' : 'bg-slate-50 text-slate-600 border-slate-200'
              ]">
                {{ kb.is_public ? '🌐 公开' : '🔒 私有' }}
              </span>
             <span :class="[
               'px-2 py-0.5 text-xs rounded-full border',
               kb.kb_type === 'storage' ? 'bg-amber-50 text-amber-600 border-amber-200' : 'bg-indigo-50 text-indigo-600 border-indigo-200'
             ]">
               {{ kb.kb_type === 'storage' ? '🗄️ 存储型' : '🧠 分析型' }}
             </span>
           </h3>
           <p class="text-sm text-slate-500 line-clamp-2 mb-3">{{ kb.description || '暂无描述' }}</p>
           <div class="mb-3 p-2 rounded-lg border border-slate-200 bg-slate-50" @click.stop>
             <div class="flex items-center justify-between gap-2">
               <label class="text-xs text-slate-600">解析优先级 (0-100)</label>
               <span class="text-xs text-slate-400" v-if="kb.kb_type === 'storage'">存储型不参与解析</span>
             </div>
             <div class="mt-2 flex items-center gap-2">
               <input
                 v-model.number="priorityDrafts[kb.id]"
                 type="number"
                 min="0"
                 max="100"
                 step="1"
                 :disabled="kb.kb_type === 'storage' || prioritySaving[kb.id] || (kb.current_user_permission !== 'editor' && kb.current_user_permission !== 'admin')"
                 class="w-24 px-2 py-1 text-sm border border-slate-200 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 disabled:bg-slate-100 disabled:text-slate-400"
               />
               <button
                 type="button"
                 :disabled="kb.kb_type === 'storage' || prioritySaving[kb.id] || (kb.current_user_permission !== 'editor' && kb.current_user_permission !== 'admin')"
                 @click="(e) => handleSaveParsePriority(e, kb)"
                 class="px-2.5 py-1 text-xs font-medium rounded-md border border-slate-200 bg-white text-slate-700 hover:border-blue-300 hover:text-blue-600 disabled:bg-slate-100 disabled:text-slate-400 disabled:border-slate-200"
               >
                 {{ prioritySaving[kb.id] ? '保存中...' : '保存' }}
               </button>
             </div>
           </div>
           <div class="flex items-center justify-between text-xs text-slate-400">
              <span>{{ kb.children_kb_count || 0 }} 个子知识库</span>
              <span>{{ kb.file_count || 0 }} 个文件</span>
           </div>
        </div>
      </div>

      <!-- Files -->
      <div class="mt-6 space-y-3" v-if="files.length > 0 || currentKb?.id !== null">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 mb-4">
          <h3 class="text-lg font-semibold text-slate-700">文件 <span class="text-sm font-normal text-slate-500">（共 {{ totalFiles }} 个）</span></h3>
          <div v-if="currentKb?.id !== null" class="flex items-center gap-2">
            <input
              v-model="fileFilterName"
              type="text"
              placeholder="文件名搜索"
              @keyup.enter="applyFileFilters"
              class="px-2.5 py-1.5 text-sm border border-slate-200 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 w-40"
            />
            <input
              v-model="fileFilterTag"
              type="text"
              placeholder="标签筛选"
              @keyup.enter="applyFileFilters"
              class="px-2.5 py-1.5 text-sm border border-slate-200 rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500 w-32"
            />
            <button
              type="button"
              @click="applyFileFilters"
              class="px-3 py-1.5 text-sm font-medium rounded-md bg-blue-600 text-white hover:bg-blue-700"
            >
              搜索
            </button>
          </div>
        </div>

        <FileCard
            v-for="file in files"
            :key="`file-${file.id}`"
            :id="`file-card-${file.id}`"
            :file="file"
            :kb-type="currentKb?.kb_type"
            :highlighted="locatedFileId === file.id"
            @updated="handleFileAction"
            @deleted="handleFileAction"
        />

        <Pagination
          v-if="totalFiles > 0"
          v-model:page="currentPage"
          v-model:size="pageSize"
          :total="totalFiles"
          @change="loadKbContent(currentKb?.id ?? null)"
        />
      </div>
    </div>

    <!-- Export Records -->
    <ExportRecordPanel
      :records="exportRecords"
      @clear="clearExportRecords"
    />

    <!-- Permission Modal -->
    <KbPermissionModal
      :kb="permissionModalKb || {}"
      :show="showPermissionModal"
      @close="showPermissionModal = false"
    />
  </div>
</template>
