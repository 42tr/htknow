<script setup>
import { ref, onMounted, computed } from 'vue'
import { api } from '../api'

const emit = defineEmits(['search', 'search-start', 'search-end'])

const query = ref('')
const error = ref('')
const selectedKbId = ref(null)
const selectedFileId = ref(null)
const knowledgeBases = ref([])
const allFiles = ref([])
const scopeSearch = ref('')
const showScopeDropdown = ref(false)
const scopeType = ref('all') // 'all', 'kb', 'file'

const loadKnowledgeBases = async () => {
  try {
    knowledgeBases.value = await api.getKnowledgeBases()
  } catch (e) {
    console.error('加载知识库列表失败:', e)
  }
}

const loadAllFiles = async () => {
  try {
    allFiles.value = await api.getFiles()
  } catch (e) {
    console.error('加载文件列表失败:', e)
  }
}

// 过滤后的知识库和文件列表
const filteredKnowledgeBases = computed(() => {
  if (!scopeSearch.value) return knowledgeBases.value
  const search = scopeSearch.value.toLowerCase()
  return knowledgeBases.value.filter(kb => 
    kb.name.toLowerCase().includes(search)
  )
})

const filteredFiles = computed(() => {
  if (!scopeSearch.value) return allFiles.value
  const search = scopeSearch.value.toLowerCase()
  return allFiles.value.filter(file => 
    file.filename.toLowerCase().includes(search)
  )
})

// 当前选择的范围描述
const scopeLabel = computed(() => {
  if (scopeType.value === 'kb' && selectedKbId.value) {
    const kb = knowledgeBases.value.find(k => k.id === selectedKbId.value)
    return kb ? `知识库: ${kb.name}` : '全部范围'
  }
  if (scopeType.value === 'file' && selectedFileId.value) {
    const file = allFiles.value.find(f => f.id === selectedFileId.value)
    return file ? `文件: ${file.filename}` : '全部范围'
  }
  return '全部范围'
})

const selectScope = (type, id = null) => {
  scopeType.value = type
  if (type === 'all') {
    selectedKbId.value = null
    selectedFileId.value = null
  } else if (type === 'kb') {
    selectedKbId.value = id
    selectedFileId.value = null
  } else if (type === 'file') {
    selectedKbId.value = null
    selectedFileId.value = id
  }
  showScopeDropdown.value = false
  scopeSearch.value = ''
}

const handleSearch = async () => {
  if (!query.value.trim()) return

  error.value = ''
  emit('search-start')

  try {
    const results = await api.search(query.value, selectedKbId.value, selectedFileId.value)
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
  loadAllFiles()
})
</script>

<template>
  <div class="max-w-2xl mx-auto">
    <!-- 搜索范围选择 -->
    <div class="mb-4 relative">
      <label class="block text-xs font-medium text-slate-600 mb-2">搜索范围</label>
      <div class="relative">
        <button
          @click="showScopeDropdown = !showScopeDropdown"
          class="w-full px-4 py-3 bg-white border border-slate-200 rounded-xl text-left flex items-center justify-between hover:border-slate-300 transition-colors"
        >
          <span class="text-slate-700">{{ scopeLabel }}</span>
          <svg class="w-5 h-5 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
          </svg>
        </button>

        <!-- 下拉菜单 -->
        <div
          v-if="showScopeDropdown"
          class="absolute z-10 w-full mt-2 bg-white border border-slate-200 rounded-xl shadow-lg max-h-96 overflow-hidden"
        >
          <!-- 搜索框 -->
          <div class="p-3 border-b border-slate-100">
            <input
              v-model="scopeSearch"
              type="text"
              placeholder="搜索知识库或文件..."
              class="w-full px-3 py-2 bg-slate-50 border border-slate-200 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
          </div>

          <div class="max-h-80 overflow-y-auto">
            <!-- 全部范围选项 -->
            <div
              @click="selectScope('all')"
              class="px-4 py-2.5 hover:bg-slate-50 cursor-pointer border-b border-slate-100"
            >
              <div class="flex items-center gap-2">
                <span class="text-2xl">🌐</span>
                <span class="text-sm font-medium text-slate-700">全部范围</span>
              </div>
            </div>

            <!-- 知识库列表 -->
            <div v-if="filteredKnowledgeBases.length > 0" class="border-b border-slate-100">
              <div class="px-4 py-2 bg-slate-50">
                <span class="text-xs font-medium text-slate-500">知识库</span>
              </div>
              <div
                v-for="kb in filteredKnowledgeBases"
                :key="'kb-' + kb.id"
                @click="selectScope('kb', kb.id)"
                class="px-4 py-2.5 hover:bg-blue-50 cursor-pointer"
              >
                <div class="flex items-center gap-2">
                  <span class="text-xl">📚</span>
                  <span class="text-sm text-slate-700">{{ kb.name }}</span>
                </div>
              </div>
            </div>

            <!-- 文件列表 -->
            <div v-if="filteredFiles.length > 0">
              <div class="px-4 py-2 bg-slate-50">
                <span class="text-xs font-medium text-slate-500">文件</span>
              </div>
              <div
                v-for="file in filteredFiles"
                :key="'file-' + file.id"
                @click="selectScope('file', file.id)"
                class="px-4 py-2.5 hover:bg-green-50 cursor-pointer"
              >
                <div class="flex items-center gap-2">
                  <span class="text-xl">📄</span>
                  <span class="text-sm text-slate-700 truncate">{{ file.filename }}</span>
                </div>
              </div>
            </div>

            <!-- 无结果 -->
            <div
              v-if="scopeSearch && filteredKnowledgeBases.length === 0 && filteredFiles.length === 0"
              class="px-4 py-8 text-center text-slate-400 text-sm"
            >
              未找到匹配的知识库或文件
            </div>
          </div>
        </div>
      </div>
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
