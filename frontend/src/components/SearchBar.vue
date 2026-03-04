<script setup>
import { computed, ref, watch } from 'vue'
import { api } from '../api'
import { currentKb } from '../store' // Import global store
import KnowledgeBaseSelector from './KnowledgeBaseSelector.vue' // Import selector

const emit = defineEmits(['search', 'search-start', 'search-end', 'advanced-search'])

const query = ref('')
const error = ref('')
const searchMode = ref('full')
const imageFile = ref(null)
const fileInput = ref(null)
const advancedOptions = ref({
  maxSteps: 3,
  docLimit: 10,
  contextChars: 2000,
  debug: false,
})
const sliceOptions = ref({
  useAdvancedFlow: false,
})

const canSubmit = computed(() => {
  if (searchMode.value === 'image') {
    return Boolean(imageFile.value)
  }
  return Boolean(query.value.trim())
})

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
  if (searchMode.value === 'advanced') {
    if (!query.value.trim()) {
      error.value = '请输入搜索内容'
      return
    }
    emit('advanced-search', {
      query: query.value.trim(),
      kbId: localSelectedKb.value?.id,
      options: { ...advancedOptions.value },
    })
    return
  }

  if (searchMode.value === 'image') {
    if (!imageFile.value) {
      error.value = '请先选择图片'
      emit('search', [])
      return
    }
  } else if (!query.value.trim()) {
    return
  }

  error.value = ''
  emit('search-start')

  try {
    let results = []
    if (searchMode.value === 'image') {
      results = await api.searchImage(imageFile.value, query.value, localSelectedKb.value?.id)
    } else {
      results =
        searchMode.value === 'slice'
          ? await api.search(query.value, localSelectedKb.value?.id, null, { advanced: sliceOptions.value.useAdvancedFlow })
          : await api.searchFull(query.value, localSelectedKb.value?.id)
    }
    emit('search', results)
  } catch (e) {
    error.value = e.message
    emit('search', [])
  } finally {
    emit('search-end')
  }
}

const handleImageChange = (e) => {
  const file = e.target.files?.[0] || null
  imageFile.value = file
}

const clearImage = () => {
  imageFile.value = null
  if (fileInput.value) {
    fileInput.value.value = ''
  }
}

const handleKeydown = (e) => {
  if (e.key === 'Enter') {
    handleSearch()
  }
}
</script>

<template>
  <div class="max-w-4xl mx-auto space-y-4">
    <!-- Search Scope Selection -->
    <div class="rounded-2xl border border-slate-200/80 bg-white/90 p-4 shadow-sm">
      <label class="block text-xs font-semibold tracking-wide text-slate-600">搜索范围</label>
      <button
        @click="showKbSelector = true"
        class="mt-2 w-full px-4 py-3 bg-white border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent text-left flex justify-between items-center hover:border-blue-300 transition-colors"
      >
        <span class="font-medium text-slate-700">{{ localSelectedKb.name }}</span>
        <svg class="w-5 h-5 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 9l4-4 4 4m0 6l-4 4-4-4" />
        </svg>
      </button>
      <p class="mt-3 text-xs text-slate-500">
        当前搜索将在 <span class="font-medium text-blue-600">{{ localSelectedKb.name }}</span> 及其子知识库中进行。
      </p>
    </div>

    <!-- Search Mode Selection -->
    <div class="rounded-2xl border border-slate-200/80 bg-white/90 p-4 shadow-sm">
      <label class="block text-xs font-semibold tracking-wide text-slate-600 mb-2">搜索模式</label>
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-2 rounded-xl border border-slate-200 bg-slate-50 p-1.5">
        <button
          type="button"
          class="px-4 py-2.5 text-sm rounded-lg transition-all duration-200"
          :class="searchMode === 'full' ? 'bg-slate-900 text-white shadow-sm' : 'text-slate-600 hover:bg-white hover:text-slate-900'"
          @click="searchMode = 'full'"
        >
          文件搜索
        </button>
        <button
          type="button"
          class="px-4 py-2.5 text-sm rounded-lg transition-all duration-200"
          :class="searchMode === 'slice' ? 'bg-slate-900 text-white shadow-sm' : 'text-slate-600 hover:bg-white hover:text-slate-900'"
          @click="searchMode = 'slice'"
        >
          切片搜索
        </button>
        <button
          type="button"
          class="px-4 py-2.5 text-sm rounded-lg transition-all duration-200"
          :class="searchMode === 'image' ? 'bg-slate-900 text-white shadow-sm' : 'text-slate-600 hover:bg-white hover:text-slate-900'"
          @click="searchMode = 'image'"
        >
          图片搜索
        </button>
        <button
          type="button"
          class="px-4 py-2.5 text-sm rounded-lg transition-all duration-200"
          :class="searchMode === 'advanced' ? 'bg-slate-900 text-white shadow-sm' : 'text-slate-600 hover:bg-white hover:text-slate-900'"
          @click="searchMode = 'advanced'"
        >
          高级搜索
        </button>
      </div>
      <p class="mt-3 text-xs text-slate-500 leading-5">
        文件搜索返回高亮片段；切片搜索返回命中的切片内容；图片搜索支持以图搜图；高级搜索以 SSE 流返回更长上下文。
      </p>
    </div>

    <!-- Knowledge Base Selector Modal -->
    <KnowledgeBaseSelector
      :show="showKbSelector"
      @close="showKbSelector = false"
      @select="handleKbSelect"
    />

    <!-- 图片搜索上传 -->
    <div v-if="searchMode === 'image'" class="space-y-4 rounded-2xl border border-slate-200/80 bg-white/90 p-4 shadow-sm">
      <div class="flex flex-wrap items-center gap-3">
        <input
          ref="fileInput"
          type="file"
          accept="image/*"
          class="hidden"
          @change="handleImageChange"
        />
        <button
          type="button"
          class="px-4 py-2.5 bg-white border border-slate-200 rounded-xl text-sm font-medium text-slate-700 hover:border-blue-300 transition-colors"
          @click="fileInput && fileInput.click()"
        >
          选择图片
        </button>
        <span class="text-sm text-slate-600 truncate max-w-xs sm:max-w-sm">
          {{ imageFile ? imageFile.name : '未选择图片' }}
        </span>
        <button
          v-if="imageFile"
          type="button"
          class="text-xs text-slate-500 hover:text-slate-700"
          @click="clearImage"
        >
          清除
        </button>
      </div>

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
          placeholder="图片描述（可选）..."
          class="w-full h-16 pl-12 pr-28 sm:pr-36 bg-white border border-slate-200 rounded-2xl text-lg shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent transition-all duration-200"
        />
        <button
          @click="handleSearch"
          :disabled="!canSubmit"
          class="absolute right-2 top-1/2 -translate-y-1/2 min-w-[88px] sm:min-w-[108px] px-5 py-2.5 bg-linear-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 transition-all duration-200 shadow-sm disabled:opacity-60 disabled:cursor-not-allowed"
        >
          搜索
        </button>
      </div>
    </div>

    <!-- 文本搜索输入框 -->
    <div v-else class="space-y-4">
      <div v-if="searchMode === 'advanced'" class="grid grid-cols-1 md:grid-cols-2 gap-4 bg-white border border-slate-200 rounded-2xl p-4">
        <div>
          <label class="block text-xs font-medium text-slate-600 mb-2">计划步骤数</label>
          <input
            type="number"
            min="1"
            max="8"
            v-model.number="advancedOptions.maxSteps"
            class="w-full px-3 py-2 border border-slate-200 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <div>
          <label class="block text-xs font-medium text-slate-600 mb-2">每步处理文档</label>
          <input
            type="number"
            min="1"
            max="20"
            v-model.number="advancedOptions.docLimit"
            class="w-full px-3 py-2 border border-slate-200 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <div>
          <label class="block text-xs font-medium text-slate-600 mb-2">上下文单侧字数</label>
          <input
            type="number"
            min="200"
            max="6000"
            step="100"
            v-model.number="advancedOptions.contextChars"
            class="w-full px-3 py-2 border border-slate-200 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <div class="flex items-center gap-3 mt-6">
          <label class="text-xs font-medium text-slate-600">输出调试信息</label>
          <button
            type="button"
            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors"
            :class="advancedOptions.debug ? 'bg-blue-600' : 'bg-slate-300'"
            @click="advancedOptions.debug = !advancedOptions.debug"
          >
            <span
              class="inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform"
              :class="advancedOptions.debug ? 'translate-x-5' : 'translate-x-1'"
            />
          </button>
        </div>
      </div>
      <div v-else-if="searchMode === 'slice'" class="bg-white border border-slate-200 rounded-2xl p-4">
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm font-semibold text-slate-700">切片高级流程</p>
            <p class="text-xs text-slate-500 mt-1 leading-5">开启后会走高级搜索判定流程，但返回仍是普通切片搜索结果格式。</p>
          </div>
          <button
            type="button"
            class="relative inline-flex h-6 w-11 items-center rounded-full transition-colors"
            :class="sliceOptions.useAdvancedFlow ? 'bg-blue-600' : 'bg-slate-300'"
            @click="sliceOptions.useAdvancedFlow = !sliceOptions.useAdvancedFlow"
          >
            <span
              class="inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform"
              :class="sliceOptions.useAdvancedFlow ? 'translate-x-5' : 'translate-x-1'"
            />
          </button>
        </div>
      </div>

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
          class="w-full h-16 pl-12 pr-28 sm:pr-36 bg-white border border-slate-200 rounded-2xl text-lg shadow-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent transition-all duration-200"
        />
        <button
          @click="handleSearch"
          :disabled="!canSubmit"
          class="absolute right-2 top-1/2 -translate-y-1/2 min-w-[88px] sm:min-w-[108px] px-5 py-2.5 bg-linear-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 transition-all duration-200 shadow-sm disabled:opacity-60 disabled:cursor-not-allowed"
        >
          搜索
        </button>
      </div>
    </div>

    <p v-if="error" class="rounded-xl border border-red-200 bg-red-50 px-4 py-2 text-center text-sm text-red-600">{{ error }}</p>
  </div>
</template>
