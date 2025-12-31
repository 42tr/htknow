<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'
import KnowledgeBaseDetail from './KnowledgeBaseDetail.vue'

const knowledgeBases = ref([])
const loading = ref(true)
const error = ref('')
const selectedKb = ref(null)
const unassignedFilesCount = ref(0)

const loadKnowledgeBases = async () => {
  loading.value = true
  error.value = ''
  try {
    knowledgeBases.value = await api.getKnowledgeBases()
    // 加载未分配知识库的文件数量
    const unassignedFiles = await api.getFiles(null)
    unassignedFilesCount.value = unassignedFiles.length
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

const handleDelete = async (e, id) => {
  e.stopPropagation()
  if (!confirm('确定要删除这个知识库吗？')) return

  try {
    await api.deleteKnowledgeBase(id)
    await loadKnowledgeBases()
  } catch (e) {
    alert('删除失败：' + e.message)
  }
}

const selectKb = (kb) => {
  selectedKb.value = kb
}

const selectUnassigned = () => {
  selectedKb.value = { id: null, name: '未分配知识库的文件', description: '这些文件尚未分配到任何知识库' }
}

const goBack = () => {
  selectedKb.value = null
  loadKnowledgeBases() // 返回时重新加载列表
}

// 暴露刷新方法给父组件
defineExpose({
  refresh: loadKnowledgeBases
})

onMounted(() => {
  loadKnowledgeBases()
})
</script>

<template>
  <div>
    <!-- Detail View -->
    <KnowledgeBaseDetail
      v-if="selectedKb"
      :kb="selectedKb"
      @back="goBack"
    />

    <!-- List View -->
    <template v-else>
      <!-- Loading -->
      <div v-if="loading" class="flex justify-center py-12">
        <div class="flex items-center gap-3 text-slate-500">
          <svg class="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
            <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
            <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
          </svg>
          <span>加载中...</span>
        </div>
      </div>

      <!-- Error -->
      <div v-else-if="error" class="text-center py-12">
        <div class="w-16 h-16 bg-red-100 rounded-full flex items-center justify-center mx-auto mb-4">
          <svg class="w-8 h-8 text-red-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
          </svg>
        </div>
        <p class="text-red-500 mb-4">{{ error }}</p>
        <button
          @click="loadKnowledgeBases"
          class="px-4 py-2 bg-slate-100 text-slate-700 rounded-lg hover:bg-slate-200 transition-colors"
        >
          重试
        </button>
      </div>

      <!-- Empty -->
      <div v-else-if="knowledgeBases.length === 0 && unassignedFilesCount === 0" class="text-center py-12">
        <div class="w-16 h-16 bg-slate-100 rounded-full flex items-center justify-center mx-auto mb-4">
          <span class="text-3xl">📚</span>
        </div>
        <p class="text-slate-500 mb-2">还没有知识库</p>
        <p class="text-sm text-slate-400">点击右上角「新建知识库」按钮创建</p>
      </div>

      <!-- Knowledge Base Grid -->
      <div v-else class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        <!-- 未分配知识库的文件卡片 -->
        <div
          v-if="unassignedFilesCount > 0"
          @click="selectUnassigned"
          class="bg-white rounded-xl p-5 border border-slate-200 hover:border-amber-300 hover:shadow-md transition-all duration-200 group cursor-pointer"
        >
          <div class="flex items-start justify-between mb-3">
            <div class="w-12 h-12 bg-gradient-to-br from-amber-100 to-orange-100 rounded-xl flex items-center justify-center">
              <span class="text-2xl">📂</span>
            </div>
          </div>

          <h3 class="font-semibold text-slate-800 mb-1">未分配的文件</h3>
          <p class="text-sm text-slate-500 line-clamp-2 mb-3">尚未分配到任何知识库的文件</p>

          <div class="flex items-center justify-between">
            <div class="flex items-center gap-3 text-xs text-slate-400">
              <span class="flex items-center gap-1">
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                </svg>
                {{ unassignedFilesCount }} 个文件
              </span>
            </div>
            <span class="text-xs text-amber-500 opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-1">
              查看详情
              <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </span>
          </div>
        </div>

        <!-- 知识库卡片 -->
        <div
          v-for="kb in knowledgeBases"
          :key="kb.id"
          @click="selectKb(kb)"
          class="bg-white rounded-xl p-5 border border-slate-200 hover:border-blue-300 hover:shadow-md transition-all duration-200 group cursor-pointer"
        >
          <div class="flex items-start justify-between mb-3">
            <div class="w-12 h-12 bg-gradient-to-br from-blue-100 to-indigo-100 rounded-xl flex items-center justify-center">
              <span class="text-2xl">📚</span>
            </div>
            <button
              @click="(e) => handleDelete(e, kb.id)"
              class="opacity-0 group-hover:opacity-100 p-2 text-slate-400 hover:text-red-500 hover:bg-red-50 rounded-lg transition-all"
              title="删除"
            >
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
              </svg>
            </button>
          </div>

          <h3 class="font-semibold text-slate-800 mb-1">{{ kb.name }}</h3>
          <p class="text-sm text-slate-500 line-clamp-2 mb-3">{{ kb.description || '暂无描述' }}</p>

          <div class="flex items-center justify-between">
            <div class="flex items-center gap-3 text-xs text-slate-400">
              <span class="flex items-center gap-1">
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
                </svg>
                {{ kb.file_count || 0 }} 个文件
              </span>
            </div>
            <span class="text-xs text-blue-500 opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-1">
              查看详情
              <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
              </svg>
            </span>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
