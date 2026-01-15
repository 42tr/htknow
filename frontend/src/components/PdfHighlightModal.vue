<script setup>
import { computed, nextTick, ref, watch } from 'vue'
import * as pdfjsLib from 'pdfjs-dist/build/pdf.mjs'
import pdfWorker from 'pdfjs-dist/build/pdf.worker.mjs?url'
import { api } from '../api'

pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorker

const props = defineProps({
  open: {
    type: Boolean,
    default: false,
  },
  file: {
    type: Object,
    default: null,
  },
  positions: {
    type: Array,
    default: () => [],
  },
})

const emit = defineEmits(['close'])

const loading = ref(false)
const error = ref('')
const pdfDoc = ref(null)
const pageViewports = ref({})
const canvasRefs = new Map()

const pages = computed(() => {
  const groups = {}
  for (const position of props.positions || []) {
    if (!position || position.page_idx === undefined || !position.bbox) {
      continue
    }
    const pageIdx = Number(position.page_idx)
    if (!groups[pageIdx]) {
      groups[pageIdx] = []
    }
    groups[pageIdx].push(position)
  }
  return Object.entries(groups)
    .map(([pageIdx, positions]) => ({
      pageIdx: Number(pageIdx),
      positions,
    }))
    .sort((a, b) => a.pageIdx - b.pageIdx)
})

const setCanvasRef = (pageIdx, el) => {
  if (el) {
    canvasRefs.set(pageIdx, el)
  }
}

const resetState = () => {
  loading.value = false
  error.value = ''
  pdfDoc.value = null
  pageViewports.value = {}
  canvasRefs.clear()
}

const loadPdf = async () => {
  if (!props.file?.id) return
  loading.value = true
  error.value = ''
  try {
    const blob = await api.getFileContent(props.file.id)
    const data = await blob.arrayBuffer()
    const loadingTask = pdfjsLib.getDocument({ data })
    pdfDoc.value = await loadingTask.promise
    await nextTick()
    await renderPages()
  } catch (err) {
    error.value = err?.message || '无法加载 PDF'
  } finally {
    loading.value = false
  }
}

const renderPagesWithOffset = async (offset) => {
  if (!pdfDoc.value) return
  let renderedCount = 0
  pageViewports.value = {}
  for (const page of pages.value) {
    const pageNumber = page.pageIdx + offset
    if (pageNumber < 1 || pageNumber > pdfDoc.value.numPages) {
      continue
    }
    const pdfPage = await pdfDoc.value.getPage(pageNumber)
    const viewport = pdfPage.getViewport({ scale: 1.3 })
    pageViewports.value[page.pageIdx] = viewport
    const canvas = canvasRefs.get(page.pageIdx)
    if (!canvas) {
      continue
    }
    const context = canvas.getContext('2d')
    canvas.width = viewport.width
    canvas.height = viewport.height
    await pdfPage.render({ canvasContext: context, viewport }).promise
    renderedCount += 1
  }
  return renderedCount
}

const renderPages = async () => {
  try {
    const rendered = await renderPagesWithOffset(1)
    if (rendered === 0) {
      await renderPagesWithOffset(0)
    }
  } catch (err) {
    error.value = err?.message || 'PDF 渲染失败'
  }
}

const getHighlightStyle = (pageIdx, bbox) => {
  if (!bbox || bbox.length < 4) return {}
  const viewport = pageViewports.value[pageIdx]
  if (!viewport) return {}
  const scale = viewport.scale
  const x1 = Math.min(bbox[0], bbox[2])
  const x2 = Math.max(bbox[0], bbox[2])
  const y1 = Math.min(bbox[1], bbox[3])
  const y2 = Math.max(bbox[1], bbox[3])
  let x = x1 * scale
  let y = y1 * scale
  const width = (x2 - x1) * scale
  const height = (y2 - y1) * scale

  if (y < 0 || y + height > viewport.height) {
    const pageHeight = viewport.height / scale
    y = (pageHeight - y2) * scale
  }

  return {
    left: `${x}px`,
    top: `${y}px`,
    width: `${width}px`,
    height: `${height}px`,
  }
}

const close = () => {
  emit('close')
}

watch(
  () => props.open,
  (open) => {
    if (open) {
      loadPdf()
    } else {
      resetState()
    }
  },
  { immediate: true }
)

watch(
  () => props.positions,
  async () => {
    if (props.open) {
      await nextTick()
      await renderPages()
    }
  },
  { deep: true }
)
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="fixed inset-0 z-100 flex items-start justify-center">
      <div class="absolute inset-0 bg-slate-900/60" @click.self="close"></div>
      <div class="relative z-10 mx-auto mt-10 max-w-5xl w-[min(100%-2rem,72rem)] bg-white rounded-2xl shadow-xl overflow-hidden">
      <div class="flex items-center justify-between px-6 py-4 border-b border-slate-200">
        <div>
          <h3 class="text-lg font-semibold text-slate-800">PDF 位置高亮</h3>
          <p class="text-sm text-slate-500">{{ file?.filename || '未知文件' }}</p>
        </div>
        <button
          class="px-3 py-1.5 rounded-lg text-sm text-slate-600 hover:text-slate-900 hover:bg-slate-100"
          @click="close"
        >
          关闭
        </button>
      </div>

      <div class="max-h-[75vh] overflow-y-auto px-6 py-5">
        <div v-if="loading" class="py-10 text-center text-slate-500">加载中...</div>
        <div v-else-if="error" class="py-10 text-center text-rose-500">{{ error }}</div>
        <div v-else-if="pages.length === 0" class="py-10 text-center text-slate-500">暂无位置信息</div>
        <div v-else class="space-y-8">
          <div v-for="page in pages" :key="page.pageIdx" class="space-y-3">
            <div class="text-sm text-slate-500">第 {{ page.pageIdx + 1 }} 页</div>
            <div class="relative inline-block shadow-sm border border-slate-200 rounded-lg overflow-hidden bg-white">
              <canvas :ref="(el) => setCanvasRef(page.pageIdx, el)"></canvas>
              <div
                v-for="(position, idx) in page.positions"
                :key="idx"
                class="absolute highlight-box"
                :style="getHighlightStyle(page.pageIdx, position.bbox)"
              ></div>
            </div>
          </div>
        </div>
      </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.highlight-box {
  border: 2px solid rgba(245, 158, 11, 0.9);
  background: rgba(251, 191, 36, 0.28);
  box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.6) inset;
  pointer-events: none;
}
</style>
