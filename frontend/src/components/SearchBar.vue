<script setup>
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '../api'
import { currentKb } from '../store'
import KnowledgeBaseSelector from './KnowledgeBaseSelector.vue'

const emit = defineEmits(['search', 'search-start', 'search-end', 'search-error'])

const query = ref('')
const busy = ref(false)
const history = ref([])
try {
  history.value = JSON.parse(
    localStorage.getItem('htknow_search_history') || '[]',
  )
    .filter((x) => typeof x === 'string')
    .slice(0, 6)
} catch {}
const error = ref('')
const imageFile = ref(null)
const imagePreview = ref('')
const canSubmit = computed(() => !busy.value && Boolean(imageFile.value || query.value.trim()))

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
  busy.value = true
  emit('search-start')

  try {
    let results = []
    if (imageFile.value) {
      results = await api.searchImage(
        imageFile.value,
        query.value,
        localSelectedKb.value?.id,
      )
    } else {
      results = await api.search(query.value, localSelectedKb.value?.id)
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

const clearImage = () => {
  if (imagePreview.value) URL.revokeObjectURL(imagePreview.value)
  imagePreview.value = ''
  imageFile.value = null
}

const handlePaste = (event) => {
  const items = Array.from(event.clipboardData?.items || [])
  const file = items.find((item) => item.type.startsWith('image/'))?.getAsFile()
  if (!file) return
  event.preventDefault()
  clearImage()
  imageFile.value = file
  imagePreview.value = URL.createObjectURL(file)
  error.value = ''
}

onBeforeUnmount(clearImage)

const handleKeydown = (e) => {
  if (e.key === 'Enter' && !e.isComposing) {
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

    <div
      class="search-composer overflow-hidden border border-slate-200 bg-white shadow-sm"
      @paste="handlePaste"
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
          :placeholder="imageFile ? '补充图片描述（可选）' : '输入关键词，或粘贴图片搜索知识库'"
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

      <div class="flex flex-wrap items-center justify-between gap-2 border-t border-slate-100 bg-slate-50/60 px-4 py-2 sm:px-5">
        <span class="text-xs text-slate-400">支持粘贴图片搜索</span>
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

      <div
        v-if="imageFile"
        class="flex items-center gap-3 border-t border-slate-100 bg-slate-50/60 px-5 py-3"
      >
        <img :src="imagePreview" alt="待搜索的图片" class="h-16 w-16 rounded-lg object-contain" />
        <span class="min-w-0 flex-1 truncate text-xs text-slate-600">
          {{ imageFile.name || '已粘贴图片' }}
        </span>
        <button type="button" class="plain-button" @click="clearImage">移除图片</button>
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
  </div>
</template>
