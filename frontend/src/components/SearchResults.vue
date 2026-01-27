<script setup>
const formatDate = (timestamp) => {
  if (!timestamp) return '-'
  return new Date(timestamp * 1000).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}

const isImageFile = (filename) => {
  if (!filename) return false
  return /\.(jpg|jpeg|png|gif|bmp|webp|tiff|tif|svg|ico|heic|heif)$/i.test(filename)
}

const isOfficeFile = (filename) => {
  if (!filename) return false
  return /\.(pdf|doc|docx|xlsx|xls)$/i.test(filename)
}

const getFileEmoji = (filename) => (isImageFile(filename) ? '🖼️' : '📄')

const canHighlight = (result) =>
  Boolean(result?.positions?.length) && isOfficeFile(result.file?.filename)

const openViewer = (result) => {
  if (!canHighlight(result)) return
  const params = new URLSearchParams({
    file_id: String(result.file?.id || ''),
    slice_id: String(result.id),
  })
  if (Array.isArray(result.positions) && result.positions.length > 0) {
    params.set('positions', btoa(JSON.stringify(result.positions)))
  }
  const url = `/pdf-highlight.html?${params.toString()}`
  window.open(url, '_blank', 'noopener')
}

defineProps({
  results: {
    type: Array,
    default: () => []
  },
  loading: {
    type: Boolean,
    default: false
  }
})
</script>

<template>
  <div class="max-w-3xl mx-auto">
    <!-- Loading State -->
    <div v-if="loading" class="flex justify-center py-12">
      <div class="flex items-center gap-3 text-slate-500">
        <svg class="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
          <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
          <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
        </svg>
        <span>搜索中...</span>
      </div>
    </div>

    <!-- Empty State -->
    <div v-else-if="results.length === 0" class="text-center py-12">
      <div class="w-16 h-16 bg-slate-100 rounded-full flex items-center justify-center mx-auto mb-4">
        <svg class="w-8 h-8 text-slate-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9.663 17h4.673M12 3v1m6.364 1.636l-.707.707M21 12h-1M4 12H3m3.343-5.657l-.707-.707m2.828 9.9a5 5 0 117.072 0l-.548.547A3.374 3.374 0 0014 18.469V19a2 2 0 11-4 0v-.531c0-.895-.356-1.754-.988-2.386l-.548-.547z" />
        </svg>
      </div>
      <p class="text-slate-500">输入关键词开始搜索</p>
    </div>

    <!-- Results -->
    <div v-else class="space-y-4">
      <p class="text-sm text-slate-500 mb-4">找到 {{ results.length }} 个结果</p>

      <div
        v-for="(result, index) in results"
        :key="index"
        class="bg-white rounded-xl p-5 border border-slate-200 hover:border-slate-300 hover:shadow-md transition-all duration-200"
        :class="canHighlight(result) ? 'cursor-pointer' : 'cursor-default'"
        @click="openViewer(result)"
      >
        <div class="flex items-start gap-4">
          <div class="w-10 h-10 bg-linear-to-br from-amber-100 to-orange-100 rounded-lg flex items-center justify-center shrink-0">
            <span class="text-lg">{{ getFileEmoji(result.file?.filename) }}</span>
          </div>
          <div class="flex-1 min-w-0">
            <h3 class="font-semibold text-slate-800 mb-1 truncate">
              {{ result.file?.filename || '未命名文档' }}
            </h3>
            <p
              v-if="result.snippet"
              class="text-slate-600 text-sm line-clamp-2 mb-2 search-snippet"
              v-html="result.snippet"
            ></p>
            <p v-else class="text-slate-600 text-sm line-clamp-2 mb-2">
              {{ result.content || (isImageFile(result.file?.filename) ? '图片匹配结果' : '无内容预览') }}
            </p>
            <div class="flex items-center gap-4 text-xs text-slate-400">
              <span class="flex items-center gap-1">
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z" />
                </svg>
                得分: {{ Number.isFinite(result.score) ? result.score.toFixed(3) : '-' }}
              </span>
              <span v-if="result.kb?.name" class="flex items-center gap-1">
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 19a2 2 0 01-2-2V7a2 2 0 012-2h4l2 2h4a2 2 0 012 2v1M5 19h14a2 2 0 002-2v-5a2 2 0 00-2-2H9a2 2 0 00-2 2v5a2 2 0 01-2 2z" />
                </svg>
                {{ result.kb.name }}
              </span>
              <span v-if="result.file?.created_at" class="flex items-center gap-1">
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                </svg>
                上传时间: {{ formatDate(result.file.created_at) }}
              </span>
            </div>
            <div v-if="canHighlight(result)" class="mt-3">
              <button
                class="inline-flex items-center gap-1 text-xs font-medium text-amber-700 bg-amber-50 px-2.5 py-1 rounded-full hover:bg-amber-100"
                @click.stop="openViewer(result)"
              >
                <span>定位原文</span>
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
                </svg>
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>

</template>

<style scoped>
.search-snippet :deep(b) {
  font-weight: 600;
  color: #b45309;
  background-color: rgba(251, 191, 36, 0.25);
  padding: 0 2px;
  border-radius: 2px;
}
</style>
