<script setup>
import { ref, onMounted, watch } from 'vue'
import { api } from '../api'
import FileCard from './FileCard.vue'

const props = defineProps({
  kb: {
    type: Object,
    required: true
  }
})

const emit = defineEmits(['back'])

const files = ref([])
const loading = ref(true)
const error = ref('')

const loadFiles = async () => {
  loading.value = true
  error.value = ''
  try {
    files.value = await api.getFiles(props.kb.id)
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

const handleFileUpdated = () => {
  loadFiles()
}

const handleFileDeleted = () => {
  loadFiles()
}

onMounted(() => {
  loadFiles()
})

watch(() => props.kb.id, () => {
  loadFiles()
})
</script>

<template>
  <div>
    <!-- Header -->
    <div class="flex items-center gap-4 mb-6">
      <button
        @click="emit('back')"
        class="p-2 text-slate-500 hover:text-slate-700 hover:bg-slate-100 rounded-lg transition-all"
      >
        <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
        </svg>
      </button>
      <div class="flex items-center gap-3">
        <div class="w-12 h-12 bg-linear-to-br from-blue-100 to-indigo-100 rounded-xl flex items-center justify-center">
          <span class="text-2xl">📚</span>
        </div>
         <div>
           <h2 class="text-xl font-bold text-slate-800 flex items-center gap-2">
             {{ kb.name }}
             <span :class="[
               'px-2 py-0.5 text-xs rounded-full border',
               kb.is_public === 1 ? 'bg-green-50 text-green-600 border-green-200' : 'bg-slate-50 text-slate-600 border-slate-200'
             ]">
               {{ kb.is_public === 1 ? '🌐 公开' : '🔒 私有' }}
             </span>
             <span :class="[
               'px-2 py-0.5 text-xs rounded-full border',
               kb.kb_type === 'storage' ? 'bg-amber-50 text-amber-600 border-amber-200' : 'bg-indigo-50 text-indigo-600 border-indigo-200'
             ]">
               {{ kb.kb_type === 'storage' ? '🗄️ 存储型' : '🧠 分析型' }}
             </span>
           </h2>
           <p class="text-sm text-slate-500">{{ kb.description || '暂无描述' }}</p>
         </div>
      </div>
    </div>

    <!-- Loading -->
    <div v-if="loading" class="flex justify-center py-12">
      <div class="flex items-center gap-3 text-slate-500">
        <svg class="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
        </svg>
        <span>加载文件列表...</span>
      </div>
    </div>

    <!-- Error -->
    <div v-else-if="error" class="text-center py-12">
      <p class="text-red-500 mb-4">{{ error }}</p>
      <button @click="loadFiles" class="px-4 py-2 bg-slate-100 text-slate-700 rounded-lg hover:bg-slate-200">
        重试
      </button>
    </div>

    <!-- Empty -->
    <div v-else-if="files.length === 0" class="text-center py-12">
      <div class="w-16 h-16 bg-slate-100 rounded-full flex items-center justify-center mx-auto mb-4">
        <span class="text-3xl">📄</span>
      </div>
      <p class="text-slate-500 mb-2">暂无文件</p>
      <p class="text-sm text-slate-400">请在「上传」页面添加文件到此知识库</p>
    </div>

    <!-- File List -->
    <div v-else class="space-y-3">
      <div class="flex items-center justify-between mb-4">
        <p class="text-sm text-slate-500">共 {{ files.length }} 个文件</p>
        <button @click="loadFiles" class="text-sm text-blue-500 hover:text-blue-600 flex items-center gap-1">
          <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
          </svg>
          刷新
        </button>
      </div>

      <FileCard
        v-for="file in files"
        :key="file.id"
        :file="file"
        :kb-type="kb.kb_type"
        @updated="handleFileUpdated"
        @deleted="handleFileDeleted"
      />
    </div>
  </div>
</template>
