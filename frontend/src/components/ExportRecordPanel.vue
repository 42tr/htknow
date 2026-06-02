<script setup>
import { ref, computed, onMounted } from 'vue'

const STORAGE_KEY = 'htknow_export_records'
const MAX_RECORDS = 50

const props = defineProps({
  records: {
    type: Array,
    default: () => [],
  },
})

const emit = defineEmits(['clear'])
const expanded = ref(false)

const formatTime = (iso) => {
  const d = new Date(iso)
  return d.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

const formatKbNames = (names) => {
  if (!names || names.length === 0) return '-'
  if (names.length <= 2) return names.join('、')
  return `${names[0]}、${names[1]} 等 ${names.length} 个`
}

const formatSize = (num) => {
  if (num >= 10000) return `${(num / 10000).toFixed(1)}万`
  return String(num)
}

const handleClear = () => {
  if (!confirm('确定要清空所有导出记录吗？')) return
  emit('clear')
}

const copyPath = async (path) => {
  try {
    await navigator.clipboard.writeText(path)
    alert('路径已复制到剪贴板')
  } catch {
    alert('复制失败')
  }
}
</script>

<template>
  <div class="mt-8 border border-slate-200 rounded-xl bg-white overflow-hidden">
    <button
      @click="expanded = !expanded"
      class="w-full px-5 py-3 flex items-center justify-between hover:bg-slate-50 transition-colors"
    >
      <div class="flex items-center gap-2">
        <svg class="w-5 h-5 text-slate-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 17v-2m3 2v-4m3 4v-6m2 10H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
        </svg>
        <span class="font-medium text-slate-700">导出记录</span>
        <span class="text-xs text-slate-400 bg-slate-100 px-2 py-0.5 rounded-full">{{ records.length }}</span>
      </div>
      <div class="flex items-center gap-2">
        <button
          v-if="records.length > 0"
          @click.stop="handleClear"
          class="text-xs text-slate-400 hover:text-red-500 px-2 py-1 rounded transition-colors"
        >
          清空
        </button>
        <svg
          class="w-5 h-5 text-slate-400 transition-transform duration-200"
          :class="expanded ? 'rotate-180' : ''"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
        </svg>
      </div>
    </button>

    <div v-if="expanded" class="border-t border-slate-100">
      <div v-if="records.length === 0" class="px-5 py-8 text-center text-slate-400 text-sm">
        暂无导出记录
      </div>
      <div v-else class="divide-y divide-slate-100 max-h-96 overflow-y-auto">
        <div
          v-for="record in records"
          :key="record.id"
          class="px-5 py-3 hover:bg-slate-50 transition-colors"
        >
          <div class="flex items-start justify-between gap-3">
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2 mb-1">
                <span class="text-xs text-slate-400">{{ formatTime(record.timestamp) }}</span>
                <span class="text-xs px-1.5 py-0.5 rounded bg-blue-50 text-blue-600 border border-blue-100">
                  {{ record.kbCount || record.kb_ids?.length || 0 }} 个知识库
                </span>
              </div>
              <div class="text-sm text-slate-700 truncate" :title="record.kbNames?.join('、')">
                {{ formatKbNames(record.kbNames) }}
              </div>
              <div class="flex items-center gap-3 mt-1.5 text-xs text-slate-400">
                <span v-if="record.fileCount">📄 {{ formatSize(record.fileCount) }} 文件</span>
                <span v-if="record.sliceCount">📑 {{ formatSize(record.sliceCount) }} 切片</span>
                <span v-if="record.tantivyDocCount">🔍 {{ formatSize(record.tantivyDocCount) }} 索引</span>
              </div>
            </div>
            <button
              @click="copyPath(record.exportPath)"
              class="shrink-0 p-1.5 text-slate-400 hover:text-blue-500 hover:bg-blue-50 rounded-lg transition-colors"
              title="复制导出路径"
            >
              <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
              </svg>
            </button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
