<script setup>
import { computed, ref, watch } from 'vue'
import { api } from '../api'
import { currentKb } from '../store'
import KnowledgeBaseSelector from './KnowledgeBaseSelector.vue'

const emit = defineEmits([
  'search',
  'search-start',
  'search-end',
  'advanced-search',
  'search-error',
])

const query = ref('')
const busy = ref(false)
const showOptions = ref(false)
const history = ref([])
try {
  history.value = JSON.parse(
    localStorage.getItem('htknow_search_history') || '[]',
  )
    .filter((x) => typeof x === 'string')
    .slice(0, 6)
} catch {}
const error = ref('')
const searchMode = ref('full')
const retrievalMode = computed({
  get: () => (searchMode.value === 'advanced' ? 'advanced' : 'normal'),
  set: (value) => {
    searchMode.value = value === 'advanced' ? 'advanced' : 'full'
  },
})
const imageFile = ref(null)
const fileInput = ref(null)
const showAdvancedPopover = ref(false)
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
  if (busy.value) return false
  if (searchMode.value === 'image') {
    return Boolean(imageFile.value)
  }
  return Boolean(query.value.trim())
})

const localSelectedKb = ref({ id: null, name: '所有知识库' })
const showKbSelector = ref(false)

watch(
  currentKb,
  (newGlobalKb) => {
    localSelectedKb.value = newGlobalKb
  },
  { immediate: true },
)

const handleKbSelect = (kb) => {
  if (kb) {
    localSelectedKb.value = kb
  } else {
    localSelectedKb.value = { id: null, name: '所有知识库' }
  }
  showKbSelector.value = false
}

const clearHistory = () => {
  history.value = []
  try {
    localStorage.removeItem('htknow_search_history')
  } catch {}
}

const handleSearch = async () => {
  if (!canSubmit.value) return
  error.value = ''
  if (query.value.trim()) {
    history.value = [
      query.value.trim(),
      ...history.value.filter((x) => x !== query.value.trim()),
    ].slice(0, 6)
    try {
      localStorage.setItem(
        'htknow_search_history',
        JSON.stringify(history.value),
      )
    } catch {}
  }
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
  busy.value = true
  emit('search-start')

  try {
    let results = []
    if (searchMode.value === 'image') {
      results = await api.searchImage(
        imageFile.value,
        query.value,
        localSelectedKb.value?.id,
      )
    } else {
      results =
        searchMode.value === 'slice'
          ? await api.search(query.value, localSelectedKb.value?.id, null, {
              advanced: sliceOptions.value.useAdvancedFlow,
            })
          : await api.searchFull(query.value, localSelectedKb.value?.id)
    }
    emit('search', results)
  } catch (e) {
    error.value = e.message
    emit('search-error')
    emit('search', [])
  } finally {
    busy.value = false
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
  if (e.key === 'Enter' && !e.isComposing) {
    handleSearch()
  }
}

const toggleAdvanced = () => {
  showAdvancedPopover.value = !showAdvancedPopover.value
}

const closeAdvanced = () => {
  showAdvancedPopover.value = false
}
</script>

<template>
  <div class="search-layout mx-auto max-w-4xl">
    <KnowledgeBaseSelector
      :show="showKbSelector"
      @close="showKbSelector = false"
      @select="handleKbSelect"
    />

    <div
      class="search-composer overflow-hidden border border-slate-200 bg-white shadow-sm"
    >
      <!-- Search input row -->
      <div class="flex items-center gap-3 px-4 py-3 sm:px-5">
        <div
          class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-slate-100 text-slate-500"
        >
          <svg
            class="h-5 w-5"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              stroke-linecap="round"
              stroke-linejoin="round"
              stroke-width="1.8"
              d="m21 21-4.35-4.35m1.35-5.15a6.5 6.5 0 1 1-13 0 6.5 6.5 0 0 1 13 0Z"
            />
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
          <svg
            class="h-5 w-5"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              stroke-linecap="round"
              stroke-linejoin="round"
              stroke-width="2"
              d="M12 19V5m-6 6 6-6 6 6"
            />
          </svg>
        </button>
      </div>

      <!-- Bottom toolbar: mode + scope + options -->
      <div class="flex flex-wrap items-center justify-between gap-2 border-t border-slate-100 bg-slate-50/60 px-4 py-2 sm:px-5">
        <!-- Search mode pills -->
        <div class="flex items-center gap-1">
          <button
            v-for="mode in [
              { id: 'full', label: '全文' },
              { id: 'slice', label: '段落' },
            ]"
            :key="mode.id"
            type="button"
            class="rounded-full px-3 py-1.5 text-xs font-medium transition"
            :class="searchMode === mode.id
              ? 'bg-slate-900 text-white'
              : 'text-slate-500 hover:bg-slate-200 hover:text-slate-700'"
            @click="searchMode = mode.id"
          >
            {{ mode.label }}
          </button>
          <span class="mx-0.5 h-4 w-px bg-slate-300" aria-hidden="true"></span>
          <button
            type="button"
            class="rounded-full px-3 py-1.5 text-xs font-medium transition"
            :class="searchMode === 'image'
              ? 'bg-slate-900 text-white'
              : 'text-slate-500 hover:bg-slate-200 hover:text-slate-700'"
            @click="searchMode = 'image'"
          >
            图片
          </button>
          <button
            type="button"
            class="relative rounded-full px-3 py-1.5 text-xs font-medium transition"
            :class="searchMode === 'advanced'
              ? 'bg-slate-900 text-white'
              : 'text-slate-500 hover:bg-slate-200 hover:text-slate-700'"
            @click="searchMode = 'advanced'"
          >
            高级
          </button>
          <!-- Advanced options gear -->
          <button
            v-if="searchMode === 'advanced'"
            type="button"
            class="ml-0.5 rounded-full p-1.5 text-slate-400 hover:bg-slate-200 hover:text-slate-600 transition"
            :class="{ 'bg-slate-200 text-slate-600': showAdvancedPopover }"
            @click="toggleAdvanced"
            title="高级选项"
          >
            <svg class="h-3.5 w-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
          </button>
          <!-- Slice options toggle -->
          <button
            v-if="searchMode === 'slice'"
            type="button"
            class="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ml-1"
            :class="sliceOptions.useAdvancedFlow ? 'bg-slate-800' : 'bg-slate-300'"
            @click="sliceOptions.useAdvancedFlow = !sliceOptions.useAdvancedFlow"
            title="段落增强检索"
          >
            <span
              class="inline-block h-4 w-4 rounded-full bg-white shadow transition-transform"
              :class="sliceOptions.useAdvancedFlow ? 'translate-x-4' : 'translate-x-0.5'"
            />
          </button>
        </div>

        <!-- Scope chip -->
        <button
          type="button"
          class="search-scope-chip"
          @click="showKbSelector = true"
        >
          <svg class="h-3 w-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
          </svg>
          {{ localSelectedKb.name }}
        </button>
      </div>

      <!-- Image upload bar -->
      <div
        v-if="searchMode === 'image'"
        class="flex flex-wrap items-center gap-3 border-t border-slate-100 bg-slate-50/60 px-5 py-3"
      >
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

      <!-- Advanced options popover -->
      <div
        v-if="searchMode === 'advanced' && showAdvancedPopover"
        class="border-t border-slate-100 bg-white px-5 py-4"
      >
        <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <label class="flex flex-col gap-1">
            <span class="text-xs text-slate-500">最大步数</span>
            <input
              v-model.number="advancedOptions.maxSteps"
              type="number"
              min="1"
              max="10"
              class="rounded-lg border border-slate-200 px-2 py-1.5 text-xs"
            />
          </label>
          <label class="flex flex-col gap-1">
            <span class="text-xs text-slate-500">文档上限</span>
            <input
              v-model.number="advancedOptions.docLimit"
              type="number"
              min="1"
              max="50"
              class="rounded-lg border border-slate-200 px-2 py-1.5 text-xs"
            />
          </label>
          <label class="flex flex-col gap-1">
            <span class="text-xs text-slate-500">上下文字符</span>
            <input
              v-model.number="advancedOptions.contextChars"
              type="number"
              min="500"
              max="10000"
              step="500"
              class="rounded-lg border border-slate-200 px-2 py-1.5 text-xs"
            />
          </label>
          <label class="flex items-end gap-2 pb-1">
            <button
              type="button"
              class="flex items-center gap-1.5 rounded-lg border border-slate-200 bg-white px-3 py-1.5 text-xs text-slate-600 transition hover:bg-slate-50"
              :class="{ 'border-slate-800 bg-slate-100': advancedOptions.debug }"
              @click="advancedOptions.debug = !advancedOptions.debug"
            >
              <span>调试信息</span>
              <span
                class="h-2 w-2 rounded-full"
                :class="advancedOptions.debug ? 'bg-emerald-500' : 'bg-slate-300'"
              ></span>
            </button>
          </label>
        </div>
      </div>
    </div>

    <!-- Error message -->
    <p
      v-if="error"
      class="mt-3 rounded-xl border border-red-200 bg-red-50 px-4 py-2 text-center text-sm text-red-600"
    >
      {{ error }}
    </p>

    <!-- Search history -->
    <div v-if="history.length" class="flex flex-wrap items-center gap-2 mt-3">
      <span class="text-xs text-slate-400">最近搜索</span>
      <button
        v-for="term in history"
        :key="term"
        class="rounded-full border border-slate-200 bg-white px-3 py-1 text-xs text-slate-600 hover:bg-slate-50 hover:border-slate-300 transition"
        @click="
          () => {
            query = term
            handleSearch()
          }
        "
      >
        {{ term }}
      </button>
      <button
        class="text-xs text-slate-400 hover:text-slate-600 transition"
        aria-label="清空搜索历史"
        @click="clearHistory()"
      >
        清空
      </button>
    </div>

    <p v-if="searchMode === 'advanced'" class="mt-2 text-xs text-slate-400 text-center">
      多步查找相关资料，耗时较长。结果将逐步展示。
    </p>
  </div>
</template>
