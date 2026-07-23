<script setup>
import { ref, computed, onMounted } from 'vue'
import { api } from '../api'
import KnowledgeBaseSelector from './KnowledgeBaseSelector.vue'

const selectedKb = ref({ id: null, name: '不分配到知识库', kb_type: 'analysis' })
const showKbSelector = ref(false)

const files = ref([])
const uploading = ref(false)
const uploadStatus = ref('')
const uploadError = ref('')
const isDragging = ref(false)
const tags = ref([])
const newTag = ref('')
const isPublic = ref(false)
const sliceType = ref('smart')
const sliceTypes = ref([])
const sliceTypesLoading = ref(true)
const sliceTypesError = ref('')
const isStorageKb = computed(() => selectedKb.value?.kb_type === 'storage')

onMounted(async () => {
  try {
    sliceTypes.value = await api.getSliceTypes()
  } catch (e) {
    sliceTypesError.value = e.message
  } finally {
    sliceTypesLoading.value = false
  }
})

const handleKbSelect = (kb) => {
  if (kb) {
    selectedKb.value = kb
  } else {
    selectedKb.value = { id: null, name: '不分配到知识库', kb_type: 'analysis' }
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

const addTag = () => {
  const tag = newTag.value.trim()
  if (tag && !tags.value.includes(tag)) {
    tags.value.push(tag)
    newTag.value = ''
  }
}

const removeTag = (index) => {
  tags.value.splice(index, 1)
}

const handleUpload = async () => {
  if (files.value.length === 0) return

  uploading.value = true
  uploadStatus.value = ''
  uploadError.value = ''

  try {
    await api.uploadFiles(selectedKb.value?.id, files.value, tags.value, isPublic.value, sliceType.value)
    uploadStatus.value = `成功上传 ${files.value.length} 个文件到 "${selectedKb.value.name}"`
    files.value = []
    tags.value = []
    isPublic.value = false
    sliceType.value = 'smart'
  } catch (e) {
    uploadError.value = e.message
  } finally {
    uploading.value = false
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto">
    <!-- Knowledge Base Select -->
    <div class="bg-white rounded-xl p-5 border border-slate-200 mb-4">
      <label class="block text-sm font-medium text-slate-700 mb-2">上传到知识库（可选）</label>
      <button
        @click="showKbSelector = true"
        class="w-full px-4 py-3 bg-slate-50 border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-left"
      >
        <span class="font-mono text-blue-600">{{ selectedKb.name }}</span>
      </button>
      <p class="mt-2 text-xs text-slate-500">
        点击以上选择文件要上传到的知识库层级
      </p>
    </div>

    <KnowledgeBaseSelector
      :show="showKbSelector"
      @close="showKbSelector = false"
      @select="handleKbSelect"
    />

    <!-- Tags Input -->
    <div class="bg-white rounded-xl p-5 border border-slate-200 mb-4">
      <label class="block text-sm font-medium text-slate-700 mb-2">文件标签（可选）</label>
      <div class="flex gap-2 mb-3">
        <input
          v-model="newTag"
          @keyup.enter="addTag"
          type="text"
          placeholder="输入标签名称..."
          class="flex-1 px-3 py-2 border border-slate-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 text-sm"
        />
        <button
          @click="addTag"
          class="px-4 py-2 bg-blue-500 text-white rounded-lg text-sm font-medium hover:bg-blue-600 transition-all"
        >
          添加
        </button>
      </div>
      <div v-if="tags.length > 0" class="flex flex-wrap gap-2">
        <div
          v-for="(tag, index) in tags"
          :key="index"
          class="flex items-center gap-1 px-3 py-1.5 bg-blue-50 text-blue-600 rounded-lg border border-blue-100 text-sm"
        >
          <span>{{ tag }}</span>
          <button
            @click="removeTag(index)"
            class="ml-1 p-0.5 text-blue-400 hover:text-red-500 hover:bg-red-50 rounded transition-all"
          >
            <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      </div>
      <p class="mt-2 text-xs text-slate-500">
        为上传的文件添加标签，方便分类和搜索
      </p>
    </div>

    <!-- Public/Private Toggle -->
    <div class="bg-white rounded-xl p-5 border border-slate-200 mb-4">
      <label class="block text-sm font-medium text-slate-700 mb-3">文件可见性</label>
      <div class="flex gap-4">
        <label
          :class="[
            'flex-1 flex items-center gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
            !isPublic ? 'border-blue-500 bg-blue-50' : 'border-slate-200 hover:border-slate-300'
          ]"
        >
          <input type="radio" v-model="isPublic" :value="false" class="w-4 h-4 text-blue-500" />
          <div>
            <div class="flex items-center gap-2 mb-1">
              <span class="text-lg">🔒</span>
              <span class="font-medium text-slate-800">私有</span>
            </div>
            <p class="text-xs text-slate-500">仅自己可见</p>
          </div>
        </label>
        <label
          :class="[
            'flex-1 flex items-center gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
            isPublic ? 'border-green-500 bg-green-50' : 'border-slate-200 hover:border-slate-300'
          ]"
        >
          <input type="radio" v-model="isPublic" :value="true" class="w-4 h-4 text-green-500" />
          <div>
            <div class="flex items-center gap-2 mb-1">
              <span class="text-lg">🌐</span>
              <span class="font-medium text-slate-800">公开</span>
            </div>
            <p class="text-xs text-slate-500">所有人可见</p>
          </div>
        </label>
      </div>
    </div>

    <!-- Slice Type Selection -->
    <div v-if="isStorageKb" class="bg-white rounded-xl p-5 border border-slate-200 mb-4">
      <label class="block text-sm font-medium text-slate-700 mb-3">切片方式</label>
      <div class="flex items-center gap-3 p-4 rounded-xl border border-amber-200 bg-amber-50 text-amber-700 text-sm">
        <span class="text-lg">🗄️</span>
        <div>
          <p class="font-medium">存储型知识库不进行解析</p>
          <p class="text-xs text-amber-600">文件将直接保存，不会生成切片或知识图谱</p>
        </div>
      </div>
    </div>
    <div v-else class="bg-white rounded-xl p-5 border border-slate-200 mb-4">
      <label class="block text-sm font-medium text-slate-700 mb-3">切片方式</label>
      <p v-if="sliceTypesLoading" class="text-sm text-slate-500">加载切片方式...</p>
      <p v-else-if="sliceTypesError" class="text-sm text-red-500">{{ sliceTypesError }}</p>
      <div v-else class="space-y-3">
        <label
          v-for="type in sliceTypes"
          :key="type.value"
          :class="[
            'flex items-start gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
            sliceType === type.value ? 'border-purple-500 bg-purple-50' : 'border-slate-200 hover:border-slate-300'
          ]"
        >
          <input type="radio" v-model="sliceType" :value="type.value" class="mt-1 w-4 h-4 text-purple-500" />
          <div class="flex-1">
            <div class="flex items-center gap-2 mb-1">
              <span class="font-medium text-slate-800">{{ type.label }}</span>
            </div>
            <p class="text-xs text-slate-500 leading-relaxed">{{ type.description }}</p>
          </div>
        </label>
      </div>
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
            <div class="w-8 h-8 bg-slate-100 rounded-lg flex items-center justify-center shrink-0">
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
          : 'bg-linear-to-r from-blue-500 to-indigo-600 text-white hover:from-blue-600 hover:to-indigo-700 shadow-sm'
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
