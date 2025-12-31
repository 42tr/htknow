<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'

const knowledgeBases = ref([])
const selectedKb = ref('')
const files = ref([])
const uploading = ref(false)
const uploadStatus = ref('')
const uploadError = ref('')
const isDragging = ref(false)

const loadKnowledgeBases = async () => {
  try {
    knowledgeBases.value = await api.getKnowledgeBases()
    // 不再自动选择第一个知识库，允许为空
  } catch (e) {
    console.error('加载知识库失败:', e)
  }
}

const handleFileSelect = (e) => {
  const selectedFiles = Array.from(e.target.files || [])
  files.value = [...files.value, ...selectedFiles]
}

const handleDrop = (e) => {
  isDragging.value = false
  const droppedFiles = Array.from(e.dataTransfer.files || [])
  files.value = [...files.value, ...droppedFiles]
}

const removeFile = (index) => {
  files.value.splice(index, 1)
}

const formatFileSize = (bytes) => {
  if (bytes === 0) return '0 B'
  const k = 1024
  const sizes = ['B', 'KB', 'MB', 'GB']
  const i = Math.floor(Math.log(bytes) / Math.log(k))
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i]
}

const handleUpload = async () => {
  if (files.value.length === 0) return

  uploading.value = true
  uploadStatus.value = ''
  uploadError.value = ''

  try {
    await api.uploadFiles(selectedKb.value || null, files.value)
    uploadStatus.value = `成功上传 ${files.value.length} 个文件`
    files.value = []
  } catch (e) {
    uploadError.value = e.message
  } finally {
    uploading.value = false
  }
}

onMounted(() => {
  loadKnowledgeBases()
})
</script>

<template>
  <div class="max-w-2xl mx-auto">
    <!-- Knowledge Base Select -->
    <div class="bg-white rounded-xl p-5 border border-slate-200 mb-4">
      <label class="block text-sm font-medium text-slate-700 mb-2">选择知识库（可选）</label>
      <select
        v-model="selectedKb"
        class="w-full px-4 py-3 bg-slate-50 border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
      >
        <option value="">不分配到知识库</option>
        <option v-for="kb in knowledgeBases" :key="kb.id" :value="kb.id">
          {{ kb.name }}
        </option>
      </select>
      <p class="mt-2 text-xs text-slate-500">
        未分配知识库的文件可以在知识库列表中查看
      </p>
    </div>

    <!-- Drop Zone -->
    <div
      @dragover.prevent="isDragging = true"
      @dragleave.prevent="isDragging = false"
      @drop.prevent="handleDrop"
      :class="[
        'bg-white rounded-xl border-2 border-dashed p-8 text-center transition-all duration-200',
        isDragging ? 'border-blue-500 bg-blue-50' : 'border-slate-200 hover:border-slate-300'
      ]"
    >
      <div class="w-16 h-16 bg-slate-100 rounded-full flex items-center justify-center mx-auto mb-4">
        <svg class="w-8 h-8 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 16a4 4 0 01-.88-7.903A5 5 0 1115.9 6L16 6a5 5 0 011 9.9M15 13l-3-3m0 0l-3 3m3-3v12" />
        </svg>
      </div>
      <p class="text-slate-600 mb-2">拖拽文件到这里，或</p>
      <label class="inline-block px-4 py-2 bg-slate-100 text-slate-700 rounded-lg hover:bg-slate-200 cursor-pointer transition-colors">
        选择文件
        <input type="file" multiple @change="handleFileSelect" class="hidden" />
      </label>
      <p class="mt-3 text-xs text-slate-400">支持 PDF、Word、TXT、Markdown 等格式</p>
    </div>

    <!-- File List -->
    <div v-if="files.length > 0" class="mt-4 bg-white rounded-xl border border-slate-200 overflow-hidden">
      <div class="p-4 border-b border-slate-100 flex items-center justify-between">
        <span class="text-sm font-medium text-slate-700">待上传文件 ({{ files.length }})</span>
        <button @click="files = []" class="text-xs text-slate-500 hover:text-red-500">
          清空
        </button>
      </div>
      <ul class="divide-y divide-slate-100">
        <li
          v-for="(file, index) in files"
          :key="index"
          class="px-4 py-3 flex items-center justify-between hover:bg-slate-50"
        >
          <div class="flex items-center gap-3 min-w-0">
            <div class="w-8 h-8 bg-slate-100 rounded-lg flex items-center justify-center flex-shrink-0">
              <span class="text-sm">📄</span>
            </div>
            <div class="min-w-0">
              <p class="text-sm text-slate-700 truncate">{{ file.name }}</p>
              <p class="text-xs text-slate-400">{{ formatFileSize(file.size) }}</p>
            </div>
          </div>
          <button
            @click="removeFile(index)"
            class="p-1.5 text-slate-400 hover:text-red-500 hover:bg-red-50 rounded-lg transition-all"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </li>
      </ul>
    </div>

    <!-- Upload Button -->
    <button
      @click="handleUpload"
      :disabled="files.length === 0 || uploading"
      :class="[
        'w-full mt-4 py-4 rounded-xl font-medium transition-all duration-200',
        files.length === 0 || uploading
          ? 'bg-slate-100 text-slate-400 cursor-not-allowed'
          : 'bg-gradient-to-r from-blue-500 to-indigo-600 text-white hover:from-blue-600 hover:to-indigo-700 shadow-sm'
      ]"
    >
      <span v-if="uploading" class="flex items-center justify-center gap-2">
        <svg class="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
        </svg>
        上传中...
      </span>
      <span v-else>上传文件</span>
    </button>

    <!-- Status Messages -->
    <p v-if="uploadStatus" class="mt-4 text-center text-green-600 text-sm">
      {{ uploadStatus }}
    </p>
    <p v-if="uploadError" class="mt-4 text-center text-red-500 text-sm">
      {{ uploadError }}
    </p>
  </div>
</template>
