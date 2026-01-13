<script setup>
import { ref, watch } from 'vue'
import { api } from '../api'
import { currentKb } from '../store' // Import global store
import KnowledgeBaseSelector from './KnowledgeBaseSelector.vue' // Import selector

const emit = defineEmits(['search', 'search-start', 'search-end'])

const query = ref('')
const error = ref('')

// Local state for search scope
const localSelectedKb = ref({ id: null, name: '所有知识库' })
const showKbSelector = ref(false)

// Initialize local scope with global context on mount/visibility
// And keep it in sync when global context changes
watch(currentKb, (newGlobalKb) => {
  localSelectedKb.value = newGlobalKb
}, { immediate: true }) // immediate: true ensures it runs on initial component setup

const handleKbSelect = (kb) => {
  if (kb) {
    localSelectedKb.value = kb
  } else {
    localSelectedKb.value = { id: null, name: '所有知识库' }
  }
  showKbSelector.value = false // Close modal after selection
}

const handleSearch = async () => {
  if (!query.value.trim()) return

  error.value = ''
  emit('search-start')

  try {
    const results = await api.search(query.value, localSelectedKb.value?.id)
    emit('search', results)
  } catch (e) {
    error.value = e.message
    emit('search', [])
  } finally {
    emit('search-end')
  }
}

const handleKeydown = (e) => {
  if (e.key === 'Enter') {
    handleSearch()
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto">
    <!-- Search Scope Selection -->
    <div class="mb-4">
      <label class="block text-xs font-medium text-slate-600 mb-2">搜索范围</label>
      <button
        @click="showKbSelector = true"
        class="w-full px-4 py-3 bg-white border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-left flex justify-between items-center hover:border-blue-300 transition-colors"
      >
        <span class="font-medium text-slate-700">{{ localSelectedKb.name }}</span>
        <svg class="w-5 h-5 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 9l4-4 4 4m0 6l-4 4-4-4" />
        </svg>
      </button>
      <p class="mt-2 text-xs text-slate-500">
        当前搜索将在 <span class="font-medium text-blue-600">{{ localSelectedKb.name }}</span> 及其子知识库中进行。
      </p>
    </div>

    <!-- Knowledge Base Selector Modal -->
    <KnowledgeBaseSelector
      :show="showKbSelector"
      @close="showKbSelector = false"
      @select="handleKbSelect"
    />

    <!-- 搜索输入框 -->
    <div class="relative">
      <div class="absolute inset-y-0 left-0 pl-4 flex items-center pointer-events-none">
        <svg class="h-5 w-5 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z" />
        </svg>
      </div>
      <input
        v-model="query"
        @keydown="handleKeydown"
        type="text"
        placeholder="输入关键词搜索..."
        class="w-full pl-12 pr-24 py-4 bg-white border border-slate-200 rounded-2xl text-lg shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent transition-all duration-200"
      />
      <button
        @click="handleSearch"
        class="absolute right-2 top-1/2 -translate-y-1/2 px-6 py-2.5 bg-linear-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 transition-all duration-200 shadow-sm"
      >
        搜索
      </button>
    </div>

    <p v-if="error" class="mt-3 text-center text-red-500 text-sm">{{ error }}</p>
  </div>
</template>
