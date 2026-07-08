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

const editingSliceId = ref(null)
const editingContent = ref('')
const saving = ref(false)

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
  if (editingSliceId.value !== null) return
  expandedSlice.value = expandedSlice.value === id ? null : id
}

const isOfficeFile = (filename) => {
  if (!filename) return false
  return /\.(pdf|doc|docx|ppt|pptx|xlsx|xls)$/i.test(filename)
}

const isExcelFile = (filename) => {
  if (!filename) return false
  return /\.(xlsx|xls)$/i.test(filename)
}

const canHighlight = () => isOfficeFile(props.file?.filename)

const openViewer = (slice) => {
  if (!canHighlight() || editingSliceId.value !== null) return

  const fileId = String(props.file.id)
  const sliceId = String(slice.id)

  // Excel 文件 → 表格预览器
  if (isExcelFile(props.file?.filename)) {
    const params = new URLSearchParams({
      file_id: fileId,
      slice_id: sliceId,
    })
    window.open(`/excel-viewer.html?${params.toString()}`, '_blank', 'noopener')
    return
  }

  // PDF/Word/PPT → PDF 高亮
  const params = new URLSearchParams({
    file_id: fileId,
    slice_id: sliceId,
  })
  const url = `/pdf-highlight.html?${params.toString()}`
  window.open(url, '_blank', 'noopener')
}

const truncateText = (text, maxLength = 200) => {
  if (!text || text.length <= maxLength) return text
  return text.substring(0, maxLength) + '...'
}

const startEdit = (slice) => {
  if (editingSliceId.value !== null && editingSliceId.value !== slice.id) {
    if (!confirm('当前有未保存的修改，是否放弃？')) return
  }
  editingSliceId.value = slice.id
  editingContent.value = slice.content
  expandedSlice.value = slice.id
}

const cancelEdit = () => {
  editingSliceId.value = null
  editingContent.value = ''
}

const saveEdit = async (slice) => {
  const content = editingContent.value.trim()
  if (!content) {
    alert('切片内容不能为空')
    return
  }

  saving.value = true
  try {
    const updatedSlices = await api.updateSlices(props.file.id, [{ id: slice.id, content }])
    const updated = updatedSlices.find((s) => s.id === slice.id)
    if (updated) {
      const idx = slices.value.findIndex((s) => s.id === slice.id)
      if (idx !== -1) {
        slices.value[idx] = updated
      }
    }
    editingSliceId.value = null
    editingContent.value = ''
  } catch (e) {
    alert('保存失败：' + e.message)
  } finally {
    saving.value = false
  }
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
                    <!-- View mode -->
                    <div v-if="editingSliceId !== slice.id">
                      <p
                        class="text-sm text-slate-700 whitespace-pre-wrap wrap-break-words cursor-pointer"
                        @click="toggleExpand(slice.id)"
                      >
                        {{ expandedSlice === slice.id ? slice.content : truncateText(slice.content) }}
                      </p>
                      <p v-if="slice.content && slice.content.length > 200" class="mt-2 text-xs text-blue-500">
                        {{ expandedSlice === slice.id ? '点击收起' : '点击展开全部' }}
                      </p>
                    </div>

                    <!-- Edit mode -->
                    <div v-else class="space-y-3">
                      <textarea
                        v-model="editingContent"
                        rows="6"
                        class="w-full p-3 text-sm text-slate-700 bg-white border border-slate-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 resize-y"
                        :disabled="saving"
                        placeholder="请输入切片内容"
                      ></textarea>
                      <div class="flex gap-3">
                        <button
                          @click="cancelEdit"
                          :disabled="saving"
                          class="flex-1 py-2 px-4 bg-slate-100 text-slate-700 rounded-lg hover:bg-slate-200 disabled:opacity-50"
                        >
                          取消
                        </button>
                        <button
                          @click="saveEdit(slice)"
                          :disabled="saving"
                          class="flex-1 py-2 px-4 bg-linear-to-r from-blue-500 to-indigo-600 text-white rounded-lg hover:from-blue-600 hover:to-indigo-700 disabled:opacity-50"
                        >
                          {{ saving ? '保存中...' : '保存' }}
                        </button>
                      </div>
                    </div>

                    <!-- Actions -->
                    <div class="mt-3 flex flex-wrap items-center gap-2">
                      <button
                        v-if="editingSliceId !== slice.id"
                        @click="startEdit(slice)"
                        class="inline-flex items-center gap-1 text-xs font-medium text-blue-700 bg-blue-50 px-2.5 py-1 rounded-full hover:bg-blue-100"
                      >
                        <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15.232 5.232l3.536 3.536m-2.036-5.036a2.5 2.5 0 113.536 3.536L6.5 21.036H3v-3.572L16.732 3.732z" />
                        </svg>
                        <span>编辑</span>
                      </button>
                      <button
                        v-if="canHighlight() && editingSliceId !== slice.id"
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
