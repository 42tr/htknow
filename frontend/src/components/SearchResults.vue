<script setup>
import { computed, ref, watch } from 'vue'
import FileDetail from './FileDetail.vue'
import WikiBrowser from './WikiBrowser.vue'
import { api } from '../api.js'
const selected = ref(null)
const sourceFile = ref(null)
const sourceError = ref('')
let sourceRequest = 0
const openWikiSource = async ({ id }) => {
  sourceError.value = ''
  const request = ++sourceRequest
  try {
    const file = await api.getFile(id)
    if (request === sourceRequest) sourceFile.value = file
  } catch (error) {
    if (request === sourceRequest) sourceError.value = error.message
  }
}
const chooseResult = (result) => {
  sourceFile.value = null
  sourceError.value = ''
  sourceRequest++
  selected.value = result
}
const kbFilter = ref('')
const typeFilter = ref('')
const sortBy = ref('relevance')

const formatDate = (timestamp) => {
  if (!timestamp) return '-'
  return new Date(timestamp * 1000).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

const isImageFile = (filename) => {
  if (!filename) return false
  return /\.(jpg|jpeg|png|gif|bmp|webp|tiff|tif|svg|ico|heic|heif)$/i.test(
    filename,
  )
}

const getFileEmoji = (filename) => (isImageFile(filename) ? 'IMG' : 'DOC')

const props = defineProps({
  searched: Boolean,
  failed: Boolean,
  results: {
    type: Array,
    default: () => [],
  },
  loading: {
    type: Boolean,
    default: false,
  },
})
const knowledgeBases = computed(() => [...new Set(props.results.map((item) => item.kb?.name).filter(Boolean))].sort())
const resultScore = (item) => Number(item.score ?? item.judge_score ?? item.relevance ?? 0)
const filteredResults = computed(() => {
  const items = props.results.filter((item) => {
    if (kbFilter.value && item.kb?.name !== kbFilter.value) return false
    if (typeFilter.value === 'image' && !isImageFile(item.file?.filename)) return false
    if (typeFilter.value === 'wiki' && !item.wiki) return false
    if (typeFilter.value === 'document' && (item.wiki || isImageFile(item.file?.filename))) return false
    return true
  })
  return [...items].sort((a, b) => sortBy.value === 'newest'
    ? Number(b.file?.created_at || 0) - Number(a.file?.created_at || 0)
    : resultScore(b) - resultScore(a))
})
const selectedIndex = computed(() => filteredResults.value.indexOf(selected.value))
const selectOffset = (offset) => {
  const next = selectedIndex.value + offset
  if (next >= 0 && next < filteredResults.value.length) chooseResult(filteredResults.value[next])
}
watch(
  () => props.results,
  () => {
    selected.value = null
    sourceFile.value = null
    sourceError.value = ''
    sourceRequest++
  },
)
</script>

<template>
  <div
    class="search-results results-workspace"
    :class="{ 'with-preview': selected?.file || sourceFile }"
  >
    <div class="min-w-0">
      <!-- Loading State -->
      <div v-if="loading" class="flex justify-center py-12">
        <div class="flex items-center gap-3 text-slate-500">
          <svg class="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
            <circle
              class="opacity-25"
              cx="12"
              cy="12"
              r="10"
              stroke="currentColor"
              stroke-width="4"
            ></circle>
            <path
              class="opacity-75"
              fill="currentColor"
              d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
            ></path>
          </svg>
          <span>搜索中...</span>
        </div>
      </div>

      <!-- Empty State -->
      <div
        v-else-if="results.length === 0 && !failed"
        class="text-center py-12"
      >
        <div
          class="w-16 h-16 bg-slate-100 rounded-full flex items-center justify-center mx-auto mb-4"
        >
          <svg
            class="w-8 h-8 text-slate-400"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              stroke-linecap="round"
              stroke-linejoin="round"
              stroke-width="2"
              d="M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z"
            />
          </svg>
        </div>
        <p class="text-slate-500">
          {{ searched ? '没有找到相关内容' : '知识就在你的资料里' }}
        </p>
        <p class="text-sm text-slate-400 mt-2">
          {{
            searched
              ? '试试更简短的关键词，或扩大知识库范围。'
              : '输入关键词，查找文档中的内容与出处。'
          }}
        </p>
      </div>

      <p v-else-if="failed" class="text-center py-12 text-slate-500">
        本次搜索未完成，请调整后重试。
      </p>

      <!-- Results -->
      <div v-else class="space-y-3">
        <div class="result-toolbar">
          <p>显示 {{ filteredResults.length }} / {{ results.length }} 个结果</p>
          <select v-model="kbFilter" aria-label="按知识库筛选"><option value="">所有知识库</option><option v-for="name in knowledgeBases" :key="name" :value="name">{{ name }}</option></select>
          <select v-model="typeFilter" aria-label="按文件类型筛选"><option value="">所有类型</option><option value="document">文档</option><option value="image">图片</option><option value="wiki">Wiki</option></select>
          <select v-model="sortBy" aria-label="结果排序"><option value="relevance">相关度优先</option><option value="newest">最新上传</option></select>
        </div>

        <div v-if="!filteredResults.length" class="empty-state">当前筛选条件下没有结果。</div>

        <div
          v-for="result in filteredResults"
          :key="result.wiki ? `wiki-${result.wiki.page.id}` : result.id || `${result.file?.id || 'file'}-${result.slice_id || result.slice_ids?.[0] || result.sliceIds?.[0] || result.receivedAt || result.file?.filename || 'result'}`"
          class="result-card group bg-white rounded-xl p-5 border border-slate-200 cursor-pointer"
          :class="{ 'is-selected': selected === result }"
          role="button"
          tabindex="0"
          :aria-label="`查看 ${result.wiki?.page.title || result.file?.filename || '未命名文档'} 的详情`"
          @click="chooseResult(result)"
          @keydown.enter="chooseResult(result)"
          @keydown.space.prevent="chooseResult(result)"
        >
          <div class="flex items-start gap-4">
            <div
              class="w-10 h-10 rounded-lg flex items-center justify-center shrink-0"
              :class="isImageFile(result.file?.filename)
                ? 'bg-linear-to-br from-purple-100 to-pink-100'
                : 'bg-linear-to-br from-amber-100 to-orange-100'"
            >
              <span
                class="text-[10px] font-semibold tracking-wide"
                :class="isImageFile(result.file?.filename) ? 'text-purple-700' : 'text-amber-700'"
              >{{ result.wiki ? 'WIKI' : getFileEmoji(result.file?.filename) }}</span>
            </div>
            <div class="flex-1 min-w-0">
              <div class="flex items-center justify-between gap-2 mb-1">
                <h3 class="font-semibold text-slate-800 truncate">
                  {{ result.wiki?.page.title || result.file?.filename || '未命名文档' }}
                </h3>
                <svg
                  class="w-4 h-4 text-slate-300 shrink-0 opacity-0 transition-opacity group-hover:opacity-100"
                  :class="selected === result ? '' : ''"
                  fill="none" stroke="currentColor" viewBox="0 0 24 24"
                >
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                </svg>
              </div>
              <p class="text-slate-600 text-sm line-clamp-2 mb-2">
                {{
                  result.content ||
                  (isImageFile(result.file?.filename)
                    ? '图片匹配结果'
                    : '无内容预览')
                }}
              </p>
              <div
                class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-400"
              >
                <span v-if="result.kb?.name" class="flex items-center gap-1">
                  <svg
                    class="w-3.5 h-3.5"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2"
                      d="M5 19a2 2 0 01-2-2V7a2 2 0 012-2h4l2 2h4a2 2 0 012 2v1M5 19h14a2 2 0 002-2v-5a2 2 0 00-2-2H9a2 2 0 00-2 2v5a2 2 0 01-2 2z"
                    />
                  </svg>
                  {{ result.kb.name }}
                </span>
                <span
                  v-if="result.file?.created_at"
                  class="flex items-center gap-1"
                >
                  <svg
                    class="w-3.5 h-3.5"
                    fill="none"
                    stroke="currentColor"
                    viewBox="0 0 24 24"
                  >
                    <path
                      stroke-linecap="round"
                      stroke-linejoin="round"
                      stroke-width="2"
                      d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z"
                    />
                  </svg>
                  {{ formatDate(result.file.created_at) }}
                </span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
    <WikiBrowser
      v-if="selected?.wiki"
      :key="`wiki-${selected.wiki.page.id}`"
      :kb-id="selected.wiki.page.kb_id"
      :initial-slug="selected.wiki.page.slug"
      :title="selected.kb?.name ? `${selected.kb.name} · Wiki` : '知识库 Wiki'"
      @locate-file="openWikiSource"
      @close="selected = null"
    />
    <p v-if="sourceError" role="alert" class="text-red-600">{{ sourceError }}</p>
    <FileDetail v-if="sourceFile" :file="sourceFile" @close="sourceFile = null" />
    <FileDetail
      v-if="selected?.file"
      :file="selected.file"
      :slice-id="selected.slice_id || selected.slice_ids?.[0] || selected.sliceIds?.[0] || selected.id"
      :position="selectedIndex + 1"
      :total="filteredResults.length"
      :has-previous="selectedIndex > 0"
      :has-next="selectedIndex >= 0 && selectedIndex < filteredResults.length - 1"
      @previous="selectOffset(-1)"
      @next="selectOffset(1)"
      @close="selected = null"
    />
  </div>
</template>
