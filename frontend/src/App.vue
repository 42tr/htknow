<script setup>
import { reactive, ref } from 'vue'
import SearchBar from './components/SearchBar.vue'
import SearchResults from './components/SearchResults.vue'
import KnowledgeBaseList from './components/KnowledgeBaseList.vue'
import FileUpload from './components/FileUpload.vue'
import KnowledgeGraph from './components/KnowledgeGraph.vue'
import AdvancedSearchPanel from './components/AdvancedSearchPanel.vue'
import { api } from './api'

const activeTab = ref('search')
const searchResults = ref([])
const isSearching = ref(false)
const advancedState = reactive({
  active: false,
  running: false,
  status: '待开始',
  planSteps: [],
  results: [],
  timeline: [],
  debugEvents: [],
  error: '',
  lastQuery: '',
})
let advancedController = null
const newId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
const sliceKey = (item) => {
  const ids = item.slice_ids || item.sliceIds
  if (Array.isArray(ids) && ids.length > 0) {
    const normalized = [...new Set(ids)].sort((a, b) => a - b)
    return normalized.join('-')
  }
  if (item.file?.id) {
    return `${item.file.id}-${item.step_action || ''}`
  }
  if (item.content) {
    return item.content.slice(0, 40)
  }
  return item.id || ''
}

const tabs = [
  { id: 'search', name: '搜索', icon: '🔍' },
  { id: 'knowledge', name: '知识库', icon: '📚' },
  { id: 'graph', name: '知识图谱', icon: '🕸️' },
  { id: 'upload', name: '上传', icon: '📤' },
]

const handleSearchResults = (results) => {
  searchResults.value = results
}

const handleSearchStart = () => {
  isSearching.value = true
}

const handleSearchEnd = () => {
  isSearching.value = false
}

const resetAdvancedState = () => {
  advancedState.active = true
  advancedState.running = true
  advancedState.status = '连接中...'
  advancedState.planSteps = []
  advancedState.results = []
  advancedState.timeline = []
  advancedState.debugEvents = []
  advancedState.error = ''
}

const pushTimeline = (entry) => {
  advancedState.timeline.unshift({ id: newId(), time: Date.now(), ...entry })
  if (advancedState.timeline.length > 50) {
    advancedState.timeline.pop()
  }
}

const pushDebug = (entry) => {
  advancedState.debugEvents.unshift({ id: newId(), time: Date.now(), ...entry })
  if (advancedState.debugEvents.length > 50) {
    advancedState.debugEvents.pop()
  }
}

const handleAdvancedSearch = ({ query, kbId, options }) => {
  if (!query) return
  if (advancedController) {
    advancedController.cancel()
  }
  resetAdvancedState()
  advancedState.lastQuery = query
  pushTimeline({ type: 'start', title: '开始搜索', message: query })

  const handlers = {
    onStatus: (payload) => {
      advancedState.status = payload?.message || payload?.phase || '处理中'
      pushTimeline({ type: 'status', title: payload?.phase || '状态', message: payload?.message || '' })
    },
    onPlan: (payload) => {
      const steps = (payload?.steps || []).map((step, index) => ({
        ...step,
        status: 'pending',
        details: null,
        index: index + 1,
      }))
      advancedState.planSteps = steps
      const message = steps.map((step) => step.comment || step.action || '').filter(Boolean).join(' → ')
      pushTimeline({ type: 'plan', title: '执行计划', message: message || '已生成计划' })
    },
    onStep: (payload) => {
      if (payload?.action) {
        const target = advancedState.planSteps.find((step) => step.action === payload.action)
        if (target) {
          target.status = payload.status || 'updated'
          target.details = payload.details || null
          if (payload.comment) {
            target.comment = payload.comment
          }
        }
      }
      pushTimeline({
        type: 'step',
        title: payload?.action ? `步骤 ${payload.action}` : '步骤更新',
        message: `${payload?.status || ''} ${payload?.comment || ''}`.trim(),
      })
    },
    onCandidate: (payload) => {
      pushDebug({ type: 'candidate', payload })
    },
    onFiltered: (payload) => {
      pushDebug({ type: 'filtered', payload })
    },
    onResult: (payload) => {
      const entry = {
        id: newId(),
        receivedAt: Date.now(),
        ...payload,
      }
      const key = sliceKey(entry)
      const existingIdx = advancedState.results.findIndex((item) => sliceKey(item) === key)
      if (existingIdx !== -1) {
        advancedState.results.splice(existingIdx, 1)
      }
      advancedState.results.unshift(entry)
      if (advancedState.results.length > 30) {
        advancedState.results.pop()
      }
    },
    onErrorEvent: (payload) => {
      advancedState.error = payload?.message || '服务返回错误'
      pushTimeline({ type: 'error', title: '错误', message: advancedState.error })
    },
    onDone: () => {
      advancedState.running = false
      advancedState.status = '已完成'
      pushTimeline({ type: 'done', title: '完成', message: '高级搜索完成' })
    },
    onError: (err) => {
      advancedState.error = err?.message || '高级搜索失败'
      advancedState.running = false
      pushTimeline({ type: 'error', title: '连接失败', message: advancedState.error })
    },
    onFinally: () => {
      advancedController = null
    },
  }

  advancedController = api.advancedSearchStream(
    {
      query,
      kbId,
      maxSubQueries: options?.maxSteps,
      perQueryLimit: options?.docLimit,
      contextChars: options?.contextChars,
      debug: options?.debug,
    },
    handlers,
  )
}

const stopAdvancedSearch = () => {
  if (advancedController) {
    advancedController.cancel()
    advancedController = null
  }
  if (advancedState.active) {
    advancedState.running = false
    advancedState.status = '已停止'
    pushTimeline({ type: 'stop', title: '已停止', message: '用户中断搜索' })
  }
}

const clearAdvanced = () => {
  advancedState.active = false
  advancedState.running = false
  advancedState.status = '待开始'
  advancedState.planSteps = []
  advancedState.results = []
  advancedState.timeline = []
  advancedState.debugEvents = []
  advancedState.error = ''
  advancedState.lastQuery = ''
}
</script>

<template>
  <div class="min-h-screen flex flex-col bg-linear-to-br from-slate-50 to-slate-100">
    <!-- Header -->
    <header class="bg-white border-b border-slate-200 sticky top-0 z-50">
      <div class="max-w-6xl mx-auto px-6 py-4">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <div class="w-10 h-10 bg-linear-to-br from-blue-500 to-indigo-600 rounded-xl flex items-center justify-center">
              <span class="text-white text-xl">📖</span>
            </div>
            <h1 class="text-xl font-semibold text-slate-800">知识库</h1>
          </div>

          <!-- Navigation -->
          <nav class="flex gap-1 bg-slate-100 p-1 rounded-xl">
            <button
              v-for="tab in tabs"
              :key="tab.id"
              @click="activeTab = tab.id"
              :class="[
                'px-4 py-2 rounded-lg text-sm font-medium transition-all duration-200',
                activeTab === tab.id
                  ? 'bg-white text-slate-900 shadow-sm'
                  : 'text-slate-600 hover:text-slate-900'
              ]"
            >
              <span class="mr-1.5">{{ tab.icon }}</span>
              {{ tab.name }}
            </button>
          </nav>
        </div>
      </div>
    </header>

    <!-- Main Content -->
    <main class="flex-1 max-w-6xl mx-auto px-6 py-8 w-full">
      <!-- Search Tab -->
      <div v-if="activeTab === 'search'" class="space-y-6">
        <div class="text-center mb-8">
          <h2 class="text-3xl font-bold text-slate-800 mb-2">搜索知识库</h2>
          <p class="text-slate-500">在所有文档中快速查找您需要的信息</p>
        </div>

        <SearchBar
          @search="handleSearchResults"
          @search-start="handleSearchStart"
          @search-end="handleSearchEnd"
          @advanced-search="handleAdvancedSearch"
        />

        <AdvancedSearchPanel
          v-if="advancedState.active"
          :state="advancedState"
          @cancel="stopAdvancedSearch"
          @clear="clearAdvanced"
        />

        <SearchResults
          :results="searchResults"
          :loading="isSearching"
        />
      </div>

      <!-- Knowledge Base Tab -->
      <div v-if="activeTab === 'knowledge'" class="space-y-6">
        <div class="flex items-center justify-between mb-6">
          <div>
            <h2 class="text-2xl font-bold text-slate-800">知识库管理</h2>
            <p class="text-slate-500 mt-1">管理您的知识库和文档</p>
          </div>
        </div>

        <KnowledgeBaseList />
      </div>

      <!-- Knowledge Graph Tab -->
      <div v-if="activeTab === 'graph'" class="space-y-6">
        <div class="text-center mb-8">
          <h2 class="text-3xl font-bold text-slate-800 mb-2">知识图谱</h2>
          <p class="text-slate-500">探索文档中的实体和关系</p>
        </div>

        <KnowledgeGraph />
      </div>

      <!-- Upload Tab -->
      <div v-if="activeTab === 'upload'" class="space-y-6">
        <div class="text-center mb-8">
          <h2 class="text-2xl font-bold text-slate-800 mb-2">上传文档</h2>
          <p class="text-slate-500">将文档添加到知识库中</p>
        </div>

        <FileUpload />
      </div>
    </main>

    <!-- Footer -->
    <footer class="border-t border-slate-200 bg-white mt-auto">
      <div class="max-w-6xl mx-auto px-6 py-4">
        <p class="text-center text-sm text-slate-500">
          知识库管理系统 · Powered by Rust + Vue
        </p>
      </div>
    </footer>
  </div>
</template>
