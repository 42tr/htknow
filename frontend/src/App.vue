<script setup>
import { vDialog } from './dialog'
import { inject, nextTick, reactive, ref, watch } from 'vue'
import SearchBar from './components/SearchBar.vue'
import SearchResults from './components/SearchResults.vue'
import KnowledgeBaseList from './components/KnowledgeBaseList.vue'
import FileUpload from './components/FileUpload.vue'
import AdvancedSearchPanel from './components/AdvancedSearchPanel.vue'
import SearchDictionaryManager from './components/SearchDictionaryManager.vue'
import { api } from './api'

const currentUser = inject('currentUser', { is_admin: false })
const logout = async () => {
  const response = await fetch('/api/auth/logout', { method: 'POST' })
  if (response.ok) window.location.assign('/?logged_out=1')
}
const activeTab = ref(
  ['search', 'knowledge', 'dictionary'].includes(location.hash.slice(1))
    ? location.hash.slice(1)
    : 'search',
)
const scrollPositions = {}
watch(activeTab, async (value, previous) => {
  scrollPositions[previous] = window.scrollY
  await nextTick()
  window.scrollTo(0, scrollPositions[value] || 0)
})
watch(activeTab, (value) => {
  history.replaceState(null, '', '#' + value)
})
const visited = reactive({ [activeTab.value]: true })
watch(activeTab, (value) => {
  visited[value] = true
})
const uploadOpen = ref(false)
const uploadBusy = ref(false)
const closeUpload = () => {
  if (!uploadBusy.value) uploadOpen.value = false
}
const kbList = ref(null)
const uploadVersion = ref(0)
const hasSearched = ref(false)
const searchFailed = ref(false)
const openUpload = () => {
  uploadVersion.value++
  uploadOpen.value = true
}
const uploaded = () => {
  kbList.value?.refresh()
}
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
let advancedRequest = 0
const newId = () =>
  `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`
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

const handleSearchResults = (results) => {
  searchResults.value = results
}

const handleSearchStart = () => {
  stopAdvancedSearch()
  clearAdvanced()
  hasSearched.value = true
  searchFailed.value = false
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
  advancedState.debugEvents.unshift({
    id: newId(),
    time: Date.now(),
    ...entry,
  })
  if (advancedState.debugEvents.length > 50) {
    advancedState.debugEvents.pop()
  }
}

const handleAdvancedSearch = ({ query, kbId, options }) => {
  if (!query) return
  const request = ++advancedRequest
  if (advancedController) {
    advancedController.cancel()
  }
  hasSearched.value = true
  searchResults.value = []
  resetAdvancedState()
  advancedState.lastQuery = query
  pushTimeline({ type: 'start', title: '开始搜索', message: query })

  const handlers = {
    onStatus: (payload) => {
      advancedState.status = payload?.message || payload?.phase || '处理中'
      pushTimeline({
        type: 'status',
        title: payload?.phase || '状态',
        message: payload?.message || '',
      })
    },
    onPlan: (payload) => {
      const steps = (payload?.steps || []).map((step, index) => ({
        ...step,
        status: 'pending',
        details: null,
        index: index + 1,
      }))
      advancedState.planSteps = steps
      const message = steps
        .map((step) => step.comment || step.action || '')
        .filter(Boolean)
        .join(' → ')
      pushTimeline({
        type: 'plan',
        title: '执行计划',
        message: message || '已生成计划',
      })
    },
    onStep: (payload) => {
      if (payload?.action) {
        const target = advancedState.planSteps.find(
          (step) => step.action === payload.action,
        )
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
      const existingIdx = advancedState.results.findIndex(
        (item) => sliceKey(item) === key,
      )
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
      pushTimeline({
        type: 'error',
        title: '错误',
        message: advancedState.error,
      })
    },
    onDone: () => {
      advancedState.running = false
      advancedState.status = '已完成'
      pushTimeline({ type: 'done', title: '完成', message: '高级搜索完成' })
    },
    onError: (err) => {
      advancedState.error = err?.message || '高级搜索失败'
      advancedState.running = false
      pushTimeline({
        type: 'error',
        title: '连接失败',
        message: advancedState.error,
      })
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
    Object.fromEntries(
      Object.entries(handlers).map(([name, callback]) => [
        name,
        (...args) => {
          if (request === advancedRequest) callback(...args)
        },
      ]),
    ),
  )
}

const stopAdvancedSearch = () => {
  advancedRequest++
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
  stopAdvancedSearch()
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
  <div class="app-shell">
    <aside class="app-sidebar">
      <a href="#search" class="brand" @click.prevent="activeTab = 'search'"
        ><span class="brand-mark">H</span
        ><span>HTKnow</span></a
      >
      <div class="sidebar-section">
        <p class="nav-caption">工作空间</p>
        <nav aria-label="主导航" class="side-nav">
          <button
            v-for="item in [
              { id: 'search', name: '搜索', icon: '⌕' },
              { id: 'knowledge', name: '知识库', icon: '▤' },
            ]"
            :key="item.id"
            :class="{ active: activeTab === item.id }"
            :aria-current="activeTab === item.id ? 'page' : undefined"
            @click="activeTab = item.id"
          >
            <span aria-hidden="true">{{ item.icon }}</span
            >{{ item.name }}
          </button>
        </nav>
      </div>
      <div class="sidebar-section">
        <p v-if="currentUser.is_admin" class="nav-caption">管理</p>
        <nav v-if="currentUser.is_admin" class="side-nav">
          <button
            :class="{ active: activeTab === 'dictionary' }"
            @click="activeTab = 'dictionary'"
          >
            <span aria-hidden="true">⚙</span>词表与同义词
          </button>
        </nav>
      </div>
      <div class="sidebar-bottom">
        <p>{{ currentUser.user_name || currentUser.user_id }}</p>
        <button @click="logout">退出当前应用</button>
        <p>让资料成为可用的知识</p>
      </div>
    </aside>
    <div class="app-body">
      <main class="app-main">
        <section
          v-if="visited.search"
          v-show="activeTab === 'search'"
          class="search-page"
          :class="{ 'has-searched': hasSearched }"
        >
          <div class="search-intro">
            <span class="eyebrow">YOUR KNOWLEDGE, CONNECTED</span>
            <h1>从资料中，找到答案的线索</h1>
            <p>搜索文档、发现关联，让每一条信息都有出处。</p>
          </div>
          <SearchBar
            @search="handleSearchResults"
            @search-start="handleSearchStart"
            @search-end="handleSearchEnd"
            @search-error="searchFailed = true"
            @advanced-search="handleAdvancedSearch"
          />
          <AdvancedSearchPanel
            v-if="advancedState.active"
            :state="advancedState"
            @cancel="stopAdvancedSearch"
            @clear="clearAdvanced"
          />
          <SearchResults
            v-else
            :results="searchResults"
            :loading="isSearching"
            :searched="hasSearched"
            :failed="searchFailed"
          />
        </section>
        <section v-if="visited.knowledge" v-show="activeTab === 'knowledge'">
          <div class="page-title">
            <div>
              <span class="eyebrow">KNOWLEDGE BASE</span>
              <h1>知识库</h1>
              <p>整理资料目录，上传文件，并从目录或文件打开知识图谱。</p>
            </div>
          </div>
          <KnowledgeBaseList ref="kbList" @upload="openUpload" />
        </section>
        <section v-if="visited.dictionary" v-show="activeTab === 'dictionary'">
          <div class="page-title">
            <div>
              <span class="eyebrow">ADMINISTRATION</span>
              <h1>词表与同义词</h1>
              <p>维护检索词表与同义词。</p>
            </div>
          </div>
          <SearchDictionaryManager />
        </section>
      </main>
    </div>
    <Teleport to="body"
      ><div
        v-if="uploadOpen"
        class="upload-overlay"
        @click.self="closeUpload"
        @keydown.esc="closeUpload"
      >
        <section
          v-dialog
          class="upload-dialog"
          role="dialog"
          aria-modal="true"
          aria-label="上传文件"
        >
          <div class="panel-heading">
            <div>
              <h2>上传文件</h2>
              <p>添加到知识库，开始整理和检索。</p>
            </div>
            <button
              class="plain-button"
              @click="closeUpload"
              :disabled="uploadBusy"
              aria-label="关闭上传"
            >
              ✕
            </button>
          </div>
          <FileUpload
            :key="uploadVersion"
            @busy="uploadBusy = $event"
            @uploaded="uploaded"
            @done="uploadOpen = false"
          />
        </section></div
    ></Teleport>
  </div>
</template>
