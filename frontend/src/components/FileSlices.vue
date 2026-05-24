<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'

const props = defineProps({
  file: {
    type: Object,
    required: true
  }
})

const emit = defineEmits(['close'])

const slices = ref([])
const loading = ref(true)
const error = ref('')
const expandedSlice = ref(null)

const loadSlices = async () => {
  loading.value = true
  error.value = ''
  try {
    slices.value = await api.getFileSlices(props.file.id)
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

const toggleExpand = (id) => {
  expandedSlice.value = expandedSlice.value === id ? null : id
}

const isOfficeFile = (filename) => {
  if (!filename) return false
  return /\.(pdf|doc|docx|xlsx|xls)$/i.test(filename)
}

const isExcelFile = (filename) => {
  if (!filename) return false
  return /\.(xlsx|xls)$/i.test(filename)
}

const canHighlight = (slice) =>
  isOfficeFile(props.file?.filename) && Array.isArray(slice?.positions) && slice.positions.length > 0

const openViewer = (slice) => {
  if (!canHighlight(slice)) return

  // Excel 文件 → 表格预览器
  if (isExcelFile(props.file?.filename)) {
    const pos = slice.positions?.[0]
    const params = new URLSearchParams({
      file_id: String(props.file.id),
      sheet_name: pos?.sheet_name || '',
      row_num: String(pos?.row_num || ''),
    })
    window.open(`/excel-viewer.html?${params.toString()}`, '_blank', 'noopener')
    return
  }

  // PDF/Word → PDF 高亮
  const params = new URLSearchParams({
    file_id: String(props.file.id),
    slice_id: String(slice.id),
  })
  if (Array.isArray(slice.positions) && slice.positions.length > 0) {
    params.set('positions', btoa(JSON.stringify(slice.positions)))
  }
  const url = `/pdf-highlight.html?${params.toString()}`
  window.open(url, '_blank', 'noopener')
}

const truncateText = (text, maxLength = 200) => {
  if (!text || text.length <= maxLength) return text
  return text.substring(0, maxLength) + '...'
}

onMounted(() => {
  loadSlices()
})
</script>

<template>
  <Teleport to="body">
    <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div @click="emit('close')" class="absolute inset-0 bg-black/50 backdrop-blur-sm"></div>

      <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-3xl max-h-[80vh] flex flex-col">
        <!-- Header -->
        <div class="flex items-center justify-between p-6 border-b border-slate-100">
          <div>
            <h3 class="text-lg font-semibold text-slate-800">文件切片</h3>
            <p class="text-sm text-slate-500">{{ file.filename }}</p>
          </div>
          <button @click="emit('close')" class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg">
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <!-- Content -->
        <div class="flex-1 overflow-y-auto p-6">
          <!-- Loading -->
          <div v-if="loading" class="flex justify-center py-12">
            <div class="flex items-center gap-3 text-slate-500">
              <svg class="animate-spin h-5 w-5" fill="none" viewBox="0 0 24 24">
                <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>
                <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"></path>
              </svg>
              <span>加载切片...</span>
            </div>
          </div>

          <!-- Error -->
          <div v-else-if="error" class="text-center py-12">
            <p class="text-red-500 mb-4">{{ error }}</p>
            <button @click="loadSlices" class="px-4 py-2 bg-slate-100 text-slate-700 rounded-lg hover:bg-slate-200">
              重试
            </button>
          </div>

          <!-- Empty -->
          <div v-else-if="slices.length === 0" class="text-center py-12">
            <p class="text-slate-500">暂无切片数据</p>
          </div>

          <!-- Slices List -->
          <div v-else class="space-y-3">
            <p class="text-sm text-slate-500 mb-4">共 {{ slices.length }} 个切片</p>

            <div
              v-for="(slice, index) in slices"
              :key="slice.id"
              class="bg-slate-50 rounded-xl border border-slate-200 overflow-hidden"
            >
              <div class="p-4 hover:bg-slate-100 transition-colors">
                <div class="flex items-start gap-3">
                  <span class="shrink-0 w-8 h-8 bg-white border border-slate-200 rounded-lg flex items-center justify-center text-sm font-medium text-slate-600">
                    {{ index + 1 }}
                  </span>
                  <div class="flex-1 min-w-0">
                    <p
                      class="text-sm text-slate-700 whitespace-pre-wrap wrap-break-words cursor-pointer"
                      @click="toggleExpand(slice.id)"
                    >
                      {{ expandedSlice === slice.id ? slice.content : truncateText(slice.content) }}
                    </p>
                    <p v-if="slice.content && slice.content.length > 200" class="mt-2 text-xs text-blue-500">
                      {{ expandedSlice === slice.id ? '点击收起' : '点击展开全部' }}
                    </p>
                    <div v-if="canHighlight(slice)" class="mt-3">
                      <button
                        class="inline-flex items-center gap-1 text-xs font-medium text-amber-700 bg-amber-50 px-2.5 py-1 rounded-full hover:bg-amber-100"
                        @click="openViewer(slice)"
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
        </div>

        <!-- Footer -->
        <div class="p-4 border-t border-slate-100">
          <button
            @click="emit('close')"
            class="w-full py-3 bg-slate-100 text-slate-700 rounded-xl font-medium hover:bg-slate-200"
          >
            关闭
          </button>
        </div>
      </div>
    </div>
  </Teleport>

</template>
