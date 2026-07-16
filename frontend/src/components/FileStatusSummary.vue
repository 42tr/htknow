<script setup>
import { computed, ref } from 'vue'

const props = defineProps({
  stats: {
    type: Object,
    default: () => ({
      total: 0,
      pending: 0,
      processing: 0,
      completed: 0,
      skipped: 0,
      failed: 0,
      unknown: 0,
    }),
  },
  loading: {
    type: Boolean,
    default: false,
  },
  retryFailedLoading: {
    type: Boolean,
    default: false,
  },
  error: {
    type: String,
    default: '',
  },
  title: {
    type: String,
    default: '文件状态概览',
  },
  subtitle: {
    type: String,
    default: '',
  },
})

const emit = defineEmits(['retry', 'reparse-failed', 'locate-file'])

const normalizedStats = computed(() => ({
  total: props.stats?.total ?? 0,
  pending: props.stats?.pending ?? 0,
  processing: props.stats?.processing ?? 0,
  completed: props.stats?.completed ?? 0,
  skipped: props.stats?.skipped ?? 0,
  failed: props.stats?.failed ?? 0,
  unknown: props.stats?.unknown ?? 0,
}))

const cardConfigs = [
  { key: 'pending', label: '待处理', bar: 'bg-amber-300', chip: 'bg-amber-50 text-amber-700' },
  { key: 'processing', label: '处理中', bar: 'bg-blue-300', chip: 'bg-blue-50 text-blue-700' },
  { key: 'completed', label: '已完成', bar: 'bg-emerald-400', chip: 'bg-emerald-50 text-emerald-700' },
  { key: 'failed', label: '处理失败', bar: 'bg-red-400', chip: 'bg-red-50 text-red-700' },
  { key: 'skipped', label: '不解析', bar: 'bg-slate-300', chip: 'bg-slate-50 text-slate-600' },
  { key: 'unknown', label: '未知状态', bar: 'bg-slate-200', chip: 'bg-slate-50 text-slate-600' },
]

const cards = computed(() => {
  return cardConfigs.filter((card) => card.key !== 'unknown' || normalizedStats.value.unknown > 0).map((card) => ({
    ...card,
    value: normalizedStats.value[card.key] ?? 0,
  }))
})

const hasData = computed(() => normalizedStats.value.total > 0)
const hasFailedFiles = computed(() => normalizedStats.value.failed > 0)
const processingFiles = computed(() => props.stats?.processing_files ?? [])
const failedFiles = computed(() => props.stats?.failed_files ?? [])
const hoveredCard = ref(null)
const selectedCard = ref(null)

const previewFiles = computed(() => {
  const key = hoveredCard.value || selectedCard.value
  if (key === 'processing') return processingFiles.value
  if (key === 'failed') return failedFiles.value
  return []
})

const activePreviewCard = computed(() => hoveredCard.value || selectedCard.value)
const previewTitle = computed(() => activePreviewCard.value === 'failed' ? '最近处理失败的文件' : '正在处理的文件')

const canPreview = (card) => {
  return (card.key === 'processing' && processingFiles.value.length > 0)
    || (card.key === 'failed' && failedFiles.value.length > 0)
}

const togglePreview = (card) => {
  if (!canPreview(card)) return
  selectedCard.value = selectedCard.value === card.key ? null : card.key
}

const getPercent = (value) => {
  if (!normalizedStats.value.total) return 0
  return Math.round((value / normalizedStats.value.total) * 100)
}

const formatTimestamp = (timestamp) => {
  if (!timestamp) return '-'
  return new Date(timestamp * 1000).toLocaleString('zh-CN')
}
</script>

<template>
  <div class="bg-white border border-slate-200 rounded-2xl p-3 shadow-sm">
    <div class="flex flex-wrap md:flex-nowrap items-center gap-3 text-sm">
      <div class="flex flex-col gap-1 min-w-[180px]">
        <span class="text-xs font-semibold text-slate-500 uppercase tracking-wide">{{ title }}</span>
        <span v-if="subtitle" class="text-[11px] text-slate-400">{{ subtitle }}</span>
        <span class="text-base font-semibold text-slate-900">
          {{ normalizedStats.total.toLocaleString() }} <span class="ml-1 text-xs text-slate-400">文件总数</span>
        </span>
        <button
          v-if="!error && !loading && hasFailedFiles"
          class="mt-1 w-fit px-2.5 py-1 text-xs font-medium rounded-lg border border-red-200 bg-red-50 text-red-700 hover:bg-red-100 disabled:opacity-60 disabled:cursor-not-allowed"
          :disabled="retryFailedLoading"
          @click="emit('reparse-failed')"
        >
          {{ retryFailedLoading ? '提交中...' : `重新解析失败文件 (${normalizedStats.failed})` }}
        </button>
      </div>

      <div class="flex-1 min-w-0">
        <div
          v-if="error"
          class="px-3 py-2 rounded-xl bg-red-50 text-red-600 text-xs flex items-center justify-between"
        >
          <span class="truncate">{{ error }}</span>
          <button class="text-red-600 underline" @click="emit('retry')">重试</button>
        </div>

        <div v-else>
          <div v-if="loading" class="flex items-center gap-2 overflow-hidden">
            <div v-for="i in 4" :key="`stats-skeleton-${i}`" class="h-10 flex-1 rounded-full bg-slate-100 animate-pulse" />
          </div>

          <div
            v-else-if="hasData"
            class="flex items-center gap-2 overflow-x-auto scrollbar-thin scrollbar-thumb-slate-300"
          >
            <div
              v-for="card in cards"
              :key="card.key"
              :class="[
                'shrink-0 px-3 py-2 rounded-2xl border border-slate-100 bg-slate-50 text-xs flex flex-col gap-0.5',
                canPreview(card)
                  ? 'cursor-pointer'
                  : ''
              ]"
              @mouseenter="hoveredCard = card.key"
              @mouseleave="hoveredCard = null"
              @click="togglePreview(card)"
            >
              <span class="text-[11px] text-slate-500 font-medium">{{ card.label }}</span>
              <span class="text-base font-semibold text-slate-900 leading-none">
                {{ card.value }}
                <span class="ml-1 text-[11px] text-slate-400">{{ getPercent(card.value) }}%</span>
              </span>
            </div>
          </div>

          <div v-else class="text-xs text-slate-400 px-3 py-2">
            暂无文件统计数据
          </div>
        </div>
      </div>
    </div>
    <div
      v-if="previewFiles.length"
      class="mt-3 bg-white border border-slate-200 rounded-2xl shadow-sm p-3 text-xs text-slate-600 w-full"
      @mouseleave="hoveredCard = null"
    >
      <div class="flex items-center justify-between text-[11px] uppercase tracking-wide text-slate-400 mb-2">
        <span>{{ previewTitle }}</span>
        <span>最近 {{ previewFiles.length }} 项</span>
      </div>
      <ul class="space-y-1">
        <li
          v-for="file in previewFiles"
          :key="file.id"
          class="border-b border-slate-100 last:border-0"
        >
          <button
            type="button"
            class="group flex w-full items-center justify-between gap-3 rounded-md px-1 py-1.5 text-left hover:bg-blue-50 focus:outline-none focus:ring-2 focus:ring-blue-300"
            :title="`前往 ${file.kb_path || file.kb_name || '未分配知识库'} 定位文件`"
            @click="emit('locate-file', file)"
          >
            <span class="min-w-0 flex-1">
              <span class="block font-medium text-slate-800 truncate group-hover:text-blue-700">{{ file.filename }}</span>
              <span class="block text-[11px] text-slate-400 group-hover:text-blue-500">
                位置：{{ file.kb_path || file.kb_name || '未分配知识库' }} · {{ activePreviewCard === 'failed' ? '失败时间' : '更新时间' }} {{ formatTimestamp(file.updated_at) }}
              </span>
            </span>
            <span class="shrink-0 text-slate-300 group-hover:text-blue-500" aria-hidden="true">›</span>
          </button>
        </li>
      </ul>
    </div>
  </div>
</template>
