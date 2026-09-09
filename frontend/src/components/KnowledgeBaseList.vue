<script setup>
import { ref, onMounted, onBeforeUnmount, computed, nextTick, watch } from 'vue'
import { api } from '../api'
import KnowledgeDirectory from './KnowledgeDirectory.vue'
import FileCard from './FileCard.vue'
import CreateKnowledgeBase from './CreateKnowledgeBase.vue'
import FileStatusSummary from './FileStatusSummary.vue'
import KnowledgeBaseExportModal from './KnowledgeBaseExportModal.vue'
import KbPermissionModal from './KbPermissionModal.vue'
import ResourceIcon from './ResourceIcon.vue'
import { setCurrentKb } from '../store'

const emit = defineEmits(['upload'])
const directoryVersion = ref(0)
const showStats = ref(false)
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
const createKnowledgeBase = ref(null)
const createParentId = ref(null)
const priorityDrafts = ref({})
const prioritySaving = ref({})
const locatedFileId = ref(null)
const listSentinel = ref(null)
const loadingMore = ref(false)
let listObserver = null

// Pagination / filter state for KB file list
const currentPage = ref(1)
const pageSize = ref(10)
const totalFiles = ref(0)
const fileFilterName = ref('')
const filtering = ref(false)
let navigationRequest = 0
const fileFilterTag = ref('')

// Pagination state for the KB grid
const kbCurrentPage = ref(1)
const kbPageSize = ref(12)
const totalKbs = ref(0)

const hasMoreKbs = computed(() => childrenKbs.value.length < totalKbs.value)
const hasMoreFiles = computed(() => files.value.length < totalFiles.value)
const hasMoreContent = computed(() => hasMoreKbs.value || hasMoreFiles.value)

// Permission modal state
const showPermissionModal = ref(false)
const permissionModalKb = ref(null)

const openPermissionModal = (kb) => {
  permissionModalKb.value = kb
  showPermissionModal.value = true
}

// Export state
const exportRecords = ref([])
const showExportModal = ref(false)
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
  const request = ++navigationRequest
  const targetId = kbId ?? null
  currentPage.value = 1
  kbCurrentPage.value = 1
  loading.value = true
  error.value = ''
  try {
    let newCurrentKb
    if (targetId === null) {
      // Root view: fetch top-level KBs and unassigned files
      const [kbData, unassignedFiles] = await Promise.all([
        api.getKnowledgeBases(null, {
          page: kbCurrentPage.value,
          size: kbPageSize.value,
        }),
        api.getFiles(null, null, {
          page: currentPage.value,
          size: pageSize.value,
        }),
      ])
      if (request !== navigationRequest) return
      childrenKbs.value = kbData.items || []
      totalKbs.value = kbData.total || 0
      files.value = unassignedFiles.items || []
      totalFiles.value = unassignedFiles.total || 0
      newCurrentKb = { id: null, name: '所有知识库', kb_type: null }
      breadcrumbs.value = []
    } else {
      // Inside a specific KB
      const [data, kbData, filesData] = await Promise.all([
        api.getKnowledgeBase(targetId),
        api.getKnowledgeBases(targetId, {
          page: kbCurrentPage.value,
          size: kbPageSize.value,
        }),
        api.getKnowledgeBaseFiles(targetId, {
          page: currentPage.value,
          size: pageSize.value,
          filename: fileFilterName.value || undefined,
          tag: fileFilterTag.value || undefined,
        }),
      ])
      if (request !== navigationRequest) return
      childrenKbs.value = kbData.items || []
      totalKbs.value = kbData.total || 0
      files.value = filesData.items || []
      totalFiles.value = filesData.total || 0
      newCurrentKb = {
        id: data.id,
        name: data.name,
        description: data.description,
        kb_type: data.kb_type,
      }
      breadcrumbs.value = data.path || []
    }
    const nextPriorityDrafts = {}
    for (const kb of childrenKbs.value) {
      nextPriorityDrafts[kb.id] = Number.isInteger(kb.parse_priority)
        ? kb.parse_priority
        : 50
    }
    priorityDrafts.value = nextPriorityDrafts
    currentKb.value = newCurrentKb
    setCurrentKb(newCurrentKb) // Update global store
    await fetchStats(targetId)
  } catch (e) {
    if (request === navigationRequest) error.value = e.message
  } finally {
    if (request === navigationRequest) loading.value = false
  }
}

const loadMoreContent = async () => {
  if (loading.value || loadingMore.value || !hasMoreContent.value) return
  loadingMore.value = true
  const request = navigationRequest
  try {
    const targetId = getCurrentKbId()
    if (hasMoreKbs.value) {
      const nextPage = kbCurrentPage.value + 1
      const data = await api.getKnowledgeBases(targetId, {
        page: nextPage,
        size: kbPageSize.value,
      })
      if (request !== navigationRequest) return
      const nextItems = data.items || []
      if (!nextItems.length) totalKbs.value = childrenKbs.value.length
      childrenKbs.value.push(...nextItems)
      kbCurrentPage.value = nextPage
      for (const kb of nextItems) {
        priorityDrafts.value[kb.id] = Number.isInteger(kb.parse_priority)
          ? kb.parse_priority
          : 50
      }
      return
    }

    const nextPage = currentPage.value + 1
    const data =
      targetId === null
        ? await api.getFiles(null, null, {
            page: nextPage,
            size: pageSize.value,
          })
        : await api.getKnowledgeBaseFiles(targetId, {
            page: nextPage,
            size: pageSize.value,
            filename: fileFilterName.value || undefined,
            tag: fileFilterTag.value || undefined,
          })
    if (request !== navigationRequest) return
    if (!data.items?.length) totalFiles.value = files.value.length
    files.value.push(...(data.items || []))
    currentPage.value = nextPage
  } catch (err) {
    error.value = err?.message || '加载更多内容失败'
  } finally {
    loadingMore.value = false
  }
}

// --- Navigation ---
const navigateToKb = (kbId) => {
  filtering.value = false
  currentPage.value = 1
  kbCurrentPage.value = 1
  fileFilterName.value = ''
  fileFilterTag.value = ''
  return loadKbContent(kbId)
}

const openUnassigned = async () => {
  await navigateToKb(null)
  await nextTick()
  document
    .querySelector('.file-section')
    ?.scrollIntoView({ behavior: 'smooth' })
}

const applyFileFilters = () => {
  filtering.value = Boolean(fileFilterName.value || fileFilterTag.value)
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
  while (
    !files.value.some((item) => item.id === file.id) &&
    hasMoreContent.value &&
    !error.value
  ) {
    await loadMoreContent()
  }
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
  directoryVersion.value++
  loadKbContent(getCurrentKbId())
}

const openCreateKnowledgeBase = async (kb) => {
  createParentId.value = kb?.id ?? null
  await nextTick()
  createKnowledgeBase.value?.open()
}

const handleDeleteKb = async (e, kbId) => {
  e.stopPropagation()
  if (!confirm('确定要删除这个知识库及其所有内容吗？此操作不可逆！')) return

  try {
    await api.deleteKnowledgeBase(kbId)
    directoryVersion.value++
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
    alert(
      count > 0
        ? `已提交 ${count} 个失败文件重新解析`
        : '当前范围内没有可重新解析的失败文件',
    )
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
  alert(
    `已提交「${kbName}」重新解析任务（含子知识库），覆盖 ${kbCount} 个知识库、${fileCount} 个文件`,
  )
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
  e?.stopPropagation?.()
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
    priorityDrafts.value[kb.id] = Number.isInteger(kb.parse_priority)
      ? kb.parse_priority
      : 50
  } finally {
    prioritySaving.value[kb.id] = false
  }
}

// Expose refresh method to parent component
defineExpose({
  refresh: () => loadKbContent(getCurrentKbId()),
})

// Initial load
onMounted(async () => {
  await loadKbContent(null)
  loadExportRecords()
  listObserver = new IntersectionObserver(
    (entries) => {
      if (entries.some((entry) => entry.isIntersecting)) loadMoreContent()
    },
    { rootMargin: '240px 0px' },
  )
  await nextTick()
  if (listSentinel.value) listObserver.observe(listSentinel.value)
})

watch(listSentinel, (el, old) => {
  if (old) listObserver?.unobserve(old)
  if (el) listObserver?.observe(el)
})
onBeforeUnmount(() => listObserver?.disconnect())
</script>

<template>
  <div class="library-layout">
    <aside class="directory-panel">
      <div class="directory-heading">资料目录</div>
      <div
        class="directory-root-row"
        :class="{ active: currentKb?.id == null }"
      >
        <button class="directory-root" @click="navigateToKb(null)">
          <ResourceIcon type="library" class="directory-root-icon" />
          <span>全部知识库</span>
        </button>
        <button
          type="button"
          class="directory-export"
          aria-label="导出知识库"
          title="导出知识库"
          @click="showExportModal = true"
        >
          <svg
            aria-hidden="true"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
          >
            <path d="M12 3v12m0 0 4-4m-4 4-4-4" />
            <path d="M5 17v3h14v-3" />
          </svg>
        </button>
      </div>
      <KnowledgeDirectory
        :key="directoryVersion"
        :selected-id="currentKb?.id"
        @select="navigateToKb"
        @create="openCreateKnowledgeBase"
        @reparse="(kb) => handleReparseChildKb(null, kb)"
      /><button class="directory-root" @click="openUnassigned">
        未分配文件
      </button>
      <CreateKnowledgeBase
        ref="createKnowledgeBase"
        :parent-id="createParentId"
        hide-trigger
        @created="handleKbCreated"
      />
    </aside>
    <div class="knowledge-workspace space-y-6">
      <button
        class="status-strip"
        :aria-expanded="showStats"
        @click="showStats = !showStats"
      >
        <span>文件状态</span><span>已完成 {{ stats.completed }}</span
        ><span>处理中 {{ stats.processing }}</span
        ><span :class="{ 'text-red-600': stats.failed }"
          >失败 {{ stats.failed }}</span
        ><span>{{ showStats ? '收起 −' : '详情 ＋' }}</span>
      </button>
      <FileStatusSummary
        v-if="showStats"
        class="workspace-status"
        :stats="stats"
        :loading="statsLoading"
        :retry-failed-loading="reparseFailedLoading"
        :error="statsError"
        :title="
          currentKb && currentKb.id !== null ? '知识库文件状态' : '全局文件状态'
        "
        :subtitle="statsSubtitle"
        @retry="fetchStats(getCurrentKbId())"
        @reparse-failed="handleReparseFailedFiles"
        @locate-file="handleLocateFile"
      />

      <!-- Loading -->
      <div
        v-if="loading"
        class="rounded-xl border border-slate-200 bg-white py-16 text-center text-sm text-slate-500"
      >
        <p>加载中...</p>
      </div>

      <!-- Error -->
      <div
        v-else-if="error"
        class="rounded-xl border border-red-200 bg-red-50 py-16 text-center text-red-600"
      >
        <p>错误: {{ error }}</p>
        <button @click="loadKbContent(getCurrentKbId())">重试</button>
      </div>

      <!-- Empty State -->
      <div
        v-else-if="childrenKbs.length === 0 && files.length === 0 && !filtering"
        class="rounded-xl border border-dashed border-slate-300 py-16 text-center"
      >
        <div
          class="w-12 h-12 bg-slate-100 rounded-xl flex items-center justify-center mx-auto mb-4"
        >
          <span class="text-xs font-semibold tracking-wide">EMPTY</span>
        </div>
        <p class="text-slate-500">这里还没有资料</p>
        <button class="primary-button mt-4" @click="emit('upload')">
          上传第一份文件
        </button>
      </div>

      <!-- Grid for KBs and Files -->
      <div
        v-else
        class="resource-stream overflow-hidden border-y border-slate-200 bg-white"
      >
        <!-- Child KBs -->
        <section
          v-if="childrenKbs.length > 0"
          class="border-b border-slate-200"
        >
          <div class="section-heading px-4 py-3 lg:px-5">
            <div>
              <h3 class="text-base font-semibold text-slate-800">知识库</h3>
              <p class="mt-1 text-xs text-slate-500">
                共 {{ totalKbs }} 个，点击进入下一级
              </p>
            </div>
          </div>
          <div class="kb-list">
            <div
              v-for="kb in childrenKbs"
              :key="`kb-${kb.id}`"
              @click="navigateToKb(kb.id)"
              class="kb-row group grid cursor-pointer gap-4 px-4 py-4 transition-colors hover:bg-slate-50 lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center lg:px-5"
            >
              <div class="min-w-0">
                <div class="min-w-0">
                  <div class="flex flex-wrap items-center gap-1.5">
                    <ResourceIcon
                      type="folder"
                      class="kb-resource-icon"
                    />
                    <h3
                      class="max-w-full truncate text-sm font-semibold text-slate-800"
                    >
                      {{ kb.name }}
                    </h3>
                    <span
                      v-if="kb.kb_type === 'storage'"
                      class="px-2 py-0.5 text-xs rounded-full border border-amber-200 bg-amber-50 text-amber-600"
                    >
                      存储型
                    </span>
                  </div>
                  <p class="mt-1 line-clamp-1 text-sm text-slate-500">
                    {{ kb.description || '暂无描述' }}
                  </p>
                  <div
                    class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-400"
                  >
                    <span>{{ kb.children_kb_count || 0 }} 个子知识库</span>
                    <span>{{ kb.file_count || 0 }} 个文件</span>
                  </div>
                </div>
              </div>

              <div
                class="flex flex-wrap items-center justify-between gap-2 lg:flex-nowrap lg:justify-end"
              >
                <div class="flex items-center gap-0.5">
                  <button
                    v-if="kb.current_user_permission === 'admin'"
                    type="button"
                    @click="(e) => handleTogglePublic(e, kb.id, kb.is_public)"
                    class="p-1.5 text-slate-400 transition hover:bg-slate-100 hover:text-slate-700"
                    :title="
                      kb.is_public
                        ? '当前公开，点击设为私有'
                        : '当前私有，点击设为公开'
                    "
                  >
                    <svg
                      v-if="kb.is_public"
                      class="h-4 w-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.8"
                        d="M7 11V7a5 5 0 0 1 9.9-1M5 11h14v9H5z"
                      />
                    </svg>
                    <svg
                      v-else
                      class="h-4 w-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <rect width="14" height="10" x="5" y="11" rx="1" />
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.8"
                        d="M8 11V7a4 4 0 0 1 8 0v4"
                      />
                    </svg>
                  </button>
                  <span
                    v-else
                    class="p-1.5 text-slate-400"
                    :title="kb.is_public ? '公开' : '私有'"
                  >
                    <svg
                      v-if="kb.is_public"
                      class="h-4 w-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.8"
                        d="M7 11V7a5 5 0 0 1 9.9-1M5 11h14v9H5z"
                      />
                    </svg>
                    <svg
                      v-else
                      class="h-4 w-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <rect width="14" height="10" x="5" y="11" rx="1" />
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.8"
                        d="M8 11V7a4 4 0 0 1 8 0v4"
                      />
                    </svg>
                  </span>
                  <label
                    class="flex items-center gap-1 px-1.5 text-slate-500"
                    title="解析优先级"
                  >
                    <svg
                      class="h-4 w-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="1.8"
                        d="M4 7h16M7 12h10M10 17h4"
                      />
                    </svg>
                    <input
                      v-model.number="priorityDrafts[kb.id]"
                      type="number"
                      min="0"
                      max="100"
                      step="1"
                      :disabled="
                        kb.kb_type === 'storage' ||
                        prioritySaving[kb.id] ||
                        (kb.current_user_permission !== 'editor' &&
                          kb.current_user_permission !== 'admin')
                      "
                      class="w-9 bg-transparent text-center text-xs font-medium text-slate-600 outline-none disabled:text-slate-300"
                      @click.stop
                      @change="(e) => handleSaveParsePriority(e, kb)"
                    />
                  </label>
                  <span
                    v-if="kb.current_user_permission"
                    class="px-2 py-0.5 text-xs rounded-full border"
                    :class="{
                      'bg-purple-50 text-purple-600 border-purple-200':
                        kb.current_user_permission === 'admin',
                      'bg-blue-50 text-blue-600 border-blue-200':
                        kb.current_user_permission === 'editor',
                      'bg-slate-50 text-slate-500 border-slate-200':
                        kb.current_user_permission === 'viewer',
                    }"
                  >
                    {{
                      kb.current_user_permission === 'admin'
                        ? '管理员'
                        : kb.current_user_permission === 'editor'
                          ? '可写'
                          : '只读'
                    }}
                  </span>
                  <button
                    v-if="kb.current_user_permission === 'admin'"
                    @click="
                      (e) => {
                        e.stopPropagation()
                        openPermissionModal(kb)
                      }
                    "
                    class="opacity-100 sm:opacity-0 sm:group-hover:opacity-100 p-1.5 text-slate-400 hover:text-purple-500 hover:bg-purple-50 rounded-md transition-all"
                    title="权限管理"
                  >
                    <svg
                      class="w-4 h-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M12 4.354a4 4 0 110 5.292M15 21H3v-1a6 6 0 0112 0v1zm0 0h6v-1a6 6 0 00-9-5.197M13 7a4 4 0 11-8 0 4 4 0 018 0z"
                      />
                    </svg>
                  </button>
                  <button
                    v-if="
                      kb.current_user_permission === 'editor' ||
                      kb.current_user_permission === 'admin'
                    "
                    @click="(e) => handleReparseChildKb(e, kb)"
                    :disabled="
                      kb.kb_type === 'storage' || childKbReparseLoading[kb.id]
                    "
                    class="opacity-100 sm:opacity-0 sm:group-hover:opacity-100 p-1.5 text-slate-400 hover:text-blue-500 hover:bg-blue-50 rounded-md transition-all disabled:opacity-40 disabled:cursor-not-allowed"
                    :title="
                      kb.kb_type === 'storage'
                        ? '存储型知识库不参与解析'
                        : childKbReparseLoading[kb.id]
                          ? '解析中...'
                          : '重新解析该知识库'
                    "
                  >
                    <svg
                      class="w-4 h-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M4 4v6h6M20 20v-6h-6M5 19a9 9 0 0014-7M19 5a9 9 0 00-14 7"
                      />
                    </svg>
                  </button>
                  <button
                    v-if="kb.current_user_permission === 'admin'"
                    @click="(e) => handleDeleteKb(e, kb.id)"
                    class="opacity-100 sm:opacity-0 sm:group-hover:opacity-100 p-1.5 text-slate-400 hover:text-red-500 hover:bg-red-50 rounded-md transition-all"
                    title="删除"
                  >
                    <svg
                      class="w-4 h-4"
                      fill="none"
                      stroke="currentColor"
                      viewBox="0 0 24 24"
                    >
                      <path
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        stroke-width="2"
                        d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
                      />
                    </svg>
                  </button>
                </div>
              </div>
            </div>
          </div>
        </section>

        <!-- Files -->
        <section
          class="file-section"
          v-if="files.length > 0 || currentKb?.id !== null"
        >
          <div
            class="section-heading flex-col gap-3 border-b border-slate-100 px-4 py-3 sm:flex-row sm:items-center sm:justify-between lg:px-5"
          >
            <div>
              <h3 class="text-base font-semibold text-slate-800">
                {{ currentKb?.id == null ? '未分配文件' : '文件' }}
              </h3>
              <p class="mt-1 text-xs text-slate-500">
                共 {{ totalFiles }} 个文件
              </p>
            </div>
            <div
              v-if="currentKb?.id !== null"
              class="flex w-full flex-wrap items-center gap-2 sm:w-auto"
            >
              <input
                v-model="fileFilterName"
                type="text"
                placeholder="文件名搜索"
                @keyup.enter="applyFileFilters"
                class="min-w-0 flex-1 px-3 py-2 text-sm border border-slate-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 sm:w-40 sm:flex-none"
              />
              <input
                v-model="fileFilterTag"
                type="text"
                placeholder="标签筛选"
                @keyup.enter="applyFileFilters"
                class="min-w-0 flex-1 px-3 py-2 text-sm border border-slate-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 sm:w-32 sm:flex-none"
              />
              <button
                type="button"
                @click="applyFileFilters"
                class="rounded-lg bg-slate-900 px-3 py-2 text-sm font-medium text-white hover:bg-slate-700"
              >
                搜索
              </button>
            </div>
          </div>

          <div class="file-list">
            <p v-if="!files.length" class="empty-state">
              没有符合条件的文件，请调整文件名或标签后重试。
            </p>
            <FileCard
              v-for="file in files"
              :key="`file-${file.id}`"
              :id="`file-card-${file.id}`"
              :file="file"
              :kb-type="currentKb?.kb_type"
              :highlighted="locatedFileId === file.id"
              flat
              @updated="handleFileAction"
              @deleted="handleFileAction"
            />
          </div>
        </section>
        <div
          ref="listSentinel"
          class="flex min-h-14 items-center justify-center border-t border-slate-100 text-xs text-slate-400"
        >
          <span v-if="loadingMore">正在加载更多内容...</span>
          <button
            v-else-if="hasMoreContent"
            class="plain-button"
            @click="loadMoreContent"
          >
            加载更多</button
          ><span v-else-if="!hasMoreContent">已加载全部内容</span>
        </div>
      </div>

      <!-- Permission Modal -->
      <KbPermissionModal
        :kb="permissionModalKb || {}"
        :show="showPermissionModal"
        @close="showPermissionModal = false"
      />

      <KnowledgeBaseExportModal
        :show="showExportModal"
        :records="exportRecords"
        @close="showExportModal = false"
        @clear-records="clearExportRecords"
        @exported="addExportRecord"
      />
    </div>
  </div>
</template>
