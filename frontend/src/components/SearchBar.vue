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
  <div class="search-layout mx-auto max-w-4xl">
    <KnowledgeBaseSelector
      :show="showKbSelector"
      @close="showKbSelector = false"
      @select="handleKbSelect"
    />

    <div class="search-composer overflow-hidden rounded-[22px] border border-slate-200 bg-white shadow-[0_12px_40px_rgba(0,0,0,0.07)]">
      <div class="flex flex-col gap-3 border-b border-slate-100 px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
        <button
          type="button"
          class="inline-flex min-w-0 items-center gap-2 self-start rounded-lg px-2.5 py-2 text-sm text-slate-600 transition hover:bg-slate-100 hover:text-slate-900"
          @click="showKbSelector = true"
        >
          <svg class="h-4 w-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="M3 7.5h6l2 2h10v9a2 2 0 01-2 2H5a2 2 0 01-2-2v-11z" />
          </svg>
          <span class="truncate font-medium">{{ localSelectedKb.name }}</span>
          <svg class="h-3.5 w-3.5 shrink-0 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="m9 7 5 5-5 5" />
          </svg>
        </button>

        <div class="flex overflow-x-auto rounded-lg bg-slate-100 p-1">
          <button
            v-for="mode in [
              { id: 'full', label: '文件' },
              { id: 'slice', label: '切片' },
              { id: 'image', label: '图片' },
              { id: 'advanced', label: '高级' },
            ]"
            :key="mode.id"
            type="button"
            class="shrink-0 rounded-md px-3 py-1.5 text-xs font-medium transition"
            :class="searchMode === mode.id ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-500 hover:text-slate-800'"
            @click="searchMode = mode.id"
          >
            {{ mode.label }}
          </button>
        </div>
      </div>

      <div v-if="searchMode === 'advanced'" class="grid grid-cols-2 gap-3 border-b border-slate-100 bg-slate-50/60 px-5 py-4 sm:grid-cols-4">
        <label class="text-xs text-slate-500">
          <span class="mb-1.5 block">计划步骤</span>
          <input v-model.number="advancedOptions.maxSteps" type="number" min="1" max="8" class="w-full rounded-lg border bg-white px-3 py-2 text-sm" />
        </label>
        <label class="text-xs text-slate-500">
          <span class="mb-1.5 block">每步文档</span>
          <input v-model.number="advancedOptions.docLimit" type="number" min="1" max="20" class="w-full rounded-lg border bg-white px-3 py-2 text-sm" />
        </label>
        <label class="text-xs text-slate-500">
          <span class="mb-1.5 block">上下文字数</span>
          <input v-model.number="advancedOptions.contextChars" type="number" min="200" max="6000" step="100" class="w-full rounded-lg border bg-white px-3 py-2 text-sm" />
        </label>
        <div class="flex items-end">
          <button type="button" class="flex h-[38px] w-full items-center justify-between rounded-lg border border-slate-200 bg-white px-3 text-xs text-slate-600" @click="advancedOptions.debug = !advancedOptions.debug">
            <span>调试信息</span>
            <span class="h-2 w-2 rounded-full" :class="advancedOptions.debug ? 'bg-emerald-500' : 'bg-slate-300'"></span>
          </button>
        </div>
      </div>

      <div v-else-if="searchMode === 'slice'" class="flex items-center justify-between gap-4 border-b border-slate-100 bg-slate-50/60 px-5 py-3">
        <div>
          <p class="text-xs font-medium text-slate-700">切片高级流程</p>
          <p class="mt-0.5 text-xs text-slate-400">使用高级判定流程返回切片结果</p>
        </div>
        <button type="button" class="relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors" :class="sliceOptions.useAdvancedFlow ? 'bg-slate-800' : 'bg-slate-300'" @click="sliceOptions.useAdvancedFlow = !sliceOptions.useAdvancedFlow">
          <span class="inline-block h-5 w-5 rounded-full bg-white shadow transition-transform" :class="sliceOptions.useAdvancedFlow ? 'translate-x-5' : 'translate-x-1'" />
        </button>
      </div>

      <div v-if="searchMode === 'image'" class="flex flex-wrap items-center gap-3 border-b border-slate-100 bg-slate-50/60 px-5 py-3">
        <input
          ref="fileInput"
          type="file"
          accept="image/*"
          class="hidden"
          @change="handleImageChange"
        />
        <button
          type="button"
          class="rounded-lg border border-slate-200 bg-white px-3 py-2 text-xs font-medium text-slate-700 transition hover:bg-slate-50"
          @click="fileInput && fileInput.click()"
        >
          选择图片
        </button>
        <span class="max-w-xs truncate text-xs text-slate-500 sm:max-w-sm">
          {{ imageFile ? imageFile.name : '未选择图片' }}
        </span>
        <button
          v-if="imageFile"
          type="button"
          class="text-xs text-slate-400 hover:text-slate-700"
          @click="clearImage"
        >
          清除
        </button>
      </div>

      <div class="flex items-center gap-3 px-4 py-4 sm:px-5">
        <div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-slate-100 text-slate-500">
          <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="m21 21-4.35-4.35m1.35-5.15a6.5 6.5 0 1 1-13 0 6.5 6.5 0 0 1 13 0Z" />
          </svg>
        </div>
        <input
          v-model="query"
          type="text"
          :placeholder="searchMode === 'image' ? '补充图片描述（可选）' : '输入关键词搜索知识库'"
          class="h-12 min-w-0 flex-1 border-0 bg-transparent px-0 text-base text-slate-800 shadow-none outline-none placeholder:text-slate-400 focus:ring-0"
          @keydown="handleKeydown"
        />
        <button
          type="button"
          :disabled="!canSubmit"
          class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-slate-900 text-white transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:bg-slate-200 disabled:text-slate-400"
          aria-label="搜索"
          @click="handleSearch"
        >
          <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 19V5m-6 6 6-6 6 6" />
          </svg>
        </button>
      </div>
    </div>

    <p class="mt-3 text-center text-xs text-slate-400">在 {{ localSelectedKb.name }} 及其子知识库中搜索</p>
    <p v-if="error" class="mt-3 rounded-xl border border-red-200 bg-red-50 px-4 py-2 text-center text-sm text-red-600">{{ error }}</p>
  </div>
</template>
