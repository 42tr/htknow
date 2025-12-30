<script setup>
import { ref } from 'vue'
import SearchBar from './components/SearchBar.vue'
import SearchResults from './components/SearchResults.vue'
import KnowledgeBaseList from './components/KnowledgeBaseList.vue'
import FileUpload from './components/FileUpload.vue'
import CreateKnowledgeBase from './components/CreateKnowledgeBase.vue'

const activeTab = ref('search')
const searchResults = ref([])
const isSearching = ref(false)
const kbListRef = ref(null)

const tabs = [
  { id: 'search', name: '搜索', icon: '🔍' },
  { id: 'knowledge', name: '知识库', icon: '📚' },
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

const handleKbCreated = () => {
  kbListRef.value?.refresh()
}
</script>

<template>
  <div class="min-h-screen flex flex-col bg-gradient-to-br from-slate-50 to-slate-100">
    <!-- Header -->
    <header class="bg-white border-b border-slate-200 sticky top-0 z-50">
      <div class="max-w-6xl mx-auto px-6 py-4">
        <div class="flex items-center justify-between">
          <div class="flex items-center gap-3">
            <div class="w-10 h-10 bg-gradient-to-br from-blue-500 to-indigo-600 rounded-xl flex items-center justify-center">
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
          <CreateKnowledgeBase @created="handleKbCreated" />
        </div>

        <KnowledgeBaseList ref="kbListRef" />
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
