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

const emit = defineEmits(['retry', 'reparse-failed'])

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
const hoveredCard = ref(null)

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
                card.key === 'processing' && processingFiles.length ? 'cursor-pointer' : ''
              ]"
              @mouseenter="hoveredCard = card.key"
              @mouseleave="hoveredCard = null"
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
      v-if="hoveredCard === 'processing' && processingFiles.length"
      class="mt-3 bg-white border border-slate-200 rounded-2xl shadow-sm p-3 text-xs text-slate-600 w-full"
      @mouseenter="hoveredCard = 'processing'"
      @mouseleave="hoveredCard = null"
    >
      <div class="flex items-center justify-between text-[11px] uppercase tracking-wide text-slate-400 mb-2">
        <span>正在处理的文件</span>
        <span>{{ processingFiles.length }} 项</span>
      </div>
      <ul class="space-y-1 max-h-60 overflow-auto pr-1">
        <li
          v-for="file in processingFiles"
          :key="file.id"
          class="flex flex-col gap-0.5 border-b border-slate-100 last:border-0 pb-1 last:pb-0"
        >
          <span class="font-medium text-slate-800 truncate">{{ file.filename }}</span>
          <span class="text-[11px] text-slate-400">
            {{ file.kb_name || '未分配知识库' }} · 更新时间 {{ formatTimestamp(file.updated_at) }}
          </span>
        </li>
      </ul>
    </div>
  </div>
</template>
