<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'
import FileCard from './FileCard.vue'
import CreateKnowledgeBase from './CreateKnowledgeBase.vue'
import { setCurrentKb } from '../store'

// Reactive state for the current view
const currentKb = ref(null) // The KB we are currently inside, null for root
const childrenKbs = ref([])
const files = ref([])
const breadcrumbs = ref([])
const loading = ref(true)
const error = ref('')

const loadKbContent = async (kbId) => {
  loading.value = true
  error.value = ''
  try {
    let newCurrentKb;
    if (kbId === null) {
      // Root view: fetch top-level KBs and unassigned files
      const [topLevelKbs, unassignedFiles] = await Promise.all([
        api.getKnowledgeBases(null),
        api.getFiles(null)
      ]);
      childrenKbs.value = topLevelKbs
      files.value = unassignedFiles
      newCurrentKb = { id: null, name: '所有知识库' }
      breadcrumbs.value = []
    } else {
      // Inside a specific KB
      const data = await api.getKnowledgeBase(kbId)
      childrenKbs.value = data.children_kbs || []
      files.value = data.files || []
      newCurrentKb = { id: data.id, name: data.name, description: data.description }
      breadcrumbs.value = data.path || []
    }
    currentKb.value = newCurrentKb;
    setCurrentKb(newCurrentKb); // Update global store
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

// --- Navigation ---
const navigateToKb = (kbId) => {
  loadKbContent(kbId)
}

// --- Event Handlers ---
const handleKbCreated = () => {
  loadKbContent(currentKb.value?.id)
}

const handleDeleteKb = async (e, kbId) => {
  e.stopPropagation()
  if (!confirm('确定要删除这个知识库及其所有内容吗？此操作不可逆！')) return

  try {
    await api.deleteKnowledgeBase(kbId)
    await loadKbContent(currentKb.value?.id) // Refresh current view
  } catch (e) {
    alert('删除失败：' + e.message)
  }
}

const handleFileAction = () => {
  loadKbContent(currentKb.value?.id)
}

// Expose refresh method to parent component
defineExpose({
  refresh: () => loadKbContent(currentKb.value?.id),
})

// Initial load
onMounted(() => {
  loadKbContent(null)
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
      <CreateKnowledgeBase :parent-id="currentKb?.id" @created="handleKbCreated" />
    </div>

    <!-- Loading -->
    <div v-if="loading" class="text-center py-12">
        <p>加载中...</p>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="text-center py-12 text-red-500">
        <p>错误: {{ error }}</p>
        <button @click="loadKbContent(currentKb?.id)">重试</button>
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
          class="bg-white rounded-xl p-5 border border-slate-200 hover:border-blue-300 hover:shadow-md transition-all duration-200 group cursor-pointer"
        >
          <div class="flex items-start justify-between mb-3">
            <div class="w-12 h-12 bg-linear-to-br from-blue-100 to-indigo-100 rounded-xl flex items-center justify-center">
              <span class="text-2xl">📚</span>
            </div>
            <button
              @click="(e) => handleDeleteKb(e, kb.id)"
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
          <div class="flex items-center justify-between text-xs text-slate-400">
             <span>{{ kb.children_kb_count || 0 }} 个子知识库</span>
             <span>{{ kb.file_count || 0 }} 个文件</span>
          </div>
        </div>
      </div>

      <!-- Files -->
      <div class="mt-6 space-y-3" v-if="files.length > 0">
        <h3 class="text-lg font-semibold text-slate-700 mb-4" v-if="childrenKbs.length > 0">文件</h3>
        <FileCard
            v-for="file in files"
            :key="`file-${file.id}`"
            :file="file"
            @updated="handleFileAction"
            @deleted="handleFileAction"
        />
      </div>
    </div>
  </div>
</template>
