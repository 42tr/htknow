<script setup>
import { vDialog } from './dialog'
import { nextTick, reactive, ref, watch } from 'vue'
import SearchBar from './components/SearchBar.vue'
import SearchResults from './components/SearchResults.vue'
import ChatPanel from './components/ChatPanel.vue'
import KnowledgeBaseList from './components/KnowledgeBaseList.vue'
import FileUpload from './components/FileUpload.vue'
import SearchDictionaryManager from './components/SearchDictionaryManager.vue'

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
const viewMode = ref('chat')
const chatPanel = ref(null)
const chatBusy = ref(false)
const handleChat = (request) => {
  viewMode.value = 'chat'
  hasSearched.value = true
  chatPanel.value?.ask(request)
}
const searchResults = ref([])
const isSearching = ref(false)
const handleSearchResults = (results) => {
  searchResults.value = results
}

const handleSearchStart = () => {
  viewMode.value = 'search'
  hasSearched.value = true
  searchFailed.value = false
  isSearching.value = true
}

const handleSearchEnd = () => {
  isSearching.value = false
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
              { id: 'search', name: '对话与搜索', icon: '⌕' },
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
        <p class="nav-caption">管理</p>
        <nav class="side-nav">
          <button
            :class="{ active: activeTab === 'dictionary' }"
            @click="activeTab = 'dictionary'"
          >
            <span aria-hidden="true">⚙</span>词表与同义词
          </button>
        </nav>
      </div>
      <div class="sidebar-bottom">
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
            <h1>向资料提问，让答案有出处</h1>
            <p>结合知识库回答问题，点击引用查看原文；也可以仅搜索资料。</p>
          </div>
          <SearchBar
            :chat-busy="chatBusy"
            @chat="handleChat"
            @search="handleSearchResults"
            @search-start="handleSearchStart"
            @search-end="handleSearchEnd"
            @search-error="searchFailed = true"
          />
          <div v-if="hasSearched" class="mx-auto max-w-4xl flex gap-3 mt-4 text-sm">
            <button class="plain-button" :aria-pressed="viewMode === 'chat'" @click="viewMode = 'chat'">对话</button>
            <button v-if="searchResults.length || viewMode === 'search'" class="plain-button" :aria-pressed="viewMode === 'search'" @click="viewMode = 'search'">搜索结果</button>
          </div>
          <ChatPanel ref="chatPanel" v-show="viewMode === 'chat'" @busy="chatBusy = $event" />
          <SearchResults
            v-show="viewMode === 'search'"
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
