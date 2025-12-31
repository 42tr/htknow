<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'

const emit = defineEmits(['search', 'search-start', 'search-end'])

const query = ref('')
const error = ref('')
const selectedKbId = ref(null)
const knowledgeBases = ref([])

const loadKnowledgeBases = async () => {
  try {
    knowledgeBases.value = await api.getKnowledgeBases()
  } catch (e) {
    console.error('加载知识库列表失败:', e)
  }
}

const handleSearch = async () => {
  if (!query.value.trim()) return

  error.value = ''
  emit('search-start')

  try {
    const results = await api.search(query.value, selectedKbId.value)
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

onMounted(() => {
  loadKnowledgeBases()
})
</script>

<template>
  <div class="max-w-2xl mx-auto">
    <!-- 知识库选择 -->
    <div class="mb-4">
      <select
        v-model="selectedKbId"
        class="w-full px-4 py-3 bg-white border border-slate-200 rounded-xl text-base shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent transition-all duration-200"
      >
        <option :value="null">全部知识库</option>
        <option v-for="kb in knowledgeBases" :key="kb.id" :value="kb.id">
          {{ kb.name }}
        </option>
      </select>
    </div>

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
        class="absolute right-2 top-1/2 -translate-y-1/2 px-6 py-2.5 bg-gradient-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 transition-all duration-200 shadow-sm"
      >
        搜索
      </button>
    </div>

    <p v-if="error" class="mt-3 text-center text-red-500 text-sm">{{ error }}</p>
  </div>
</template>
