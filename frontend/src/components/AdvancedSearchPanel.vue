<script setup>
import { computed, ref } from 'vue'

const props = defineProps({
  state: {
    type: Object,
    required: true,
  },
})
const state = props.state
const emit = defineEmits(['cancel', 'clear'])
const showDebug = ref(false)

const hasResults = computed(() => (state?.results || []).length > 0)

const formatTime = (timestamp) => {
  if (!timestamp) return ''
  return new Date(timestamp).toLocaleTimeString('zh-CN', {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

const copyContent = async (text) => {
  if (!text) return
  try {
    await navigator.clipboard.writeText(text)
  } catch (err) {
    console.error('copy failed', err)
  }
}
</script>

<template>
  <div class="bg-white border border-slate-200 rounded-2xl p-6 shadow-sm mb-6">
    <div class="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
      <div>
        <div class="flex items-center gap-2">
          <h3 class="text-lg font-semibold text-slate-800">高级搜索流</h3>
          <span
            class="inline-flex items-center gap-1 text-xs font-medium px-2 py-0.5 rounded-full"
            :class="state.running ? 'bg-green-50 text-green-700' : 'bg-slate-100 text-slate-600'"
          >
            <span class="h-2 w-2 rounded-full" :class="state.running ? 'bg-green-500 animate-pulse' : 'bg-slate-400'"></span>
            {{ state.running ? '进行中' : '已结束' }}
          </span>
        </div>
        <p class="text-sm text-slate-500 mt-1">
          状态：{{ state.status }}
        </p>
      </div>
      <div class="flex gap-3">
        <button
          v-if="state.running"
          type="button"
          class="px-4 py-2 rounded-xl border border-slate-200 text-sm font-medium hover:bg-slate-50"
          @click="$emit('cancel')"
        >
          停止
        </button>
        <button
          type="button"
          class="px-4 py-2 rounded-xl border border-slate-200 text-sm font-medium hover:bg-slate-50"
          @click="$emit('clear')"
        >
          清除
        </button>
      </div>
    </div>

    <div v-if="state.error" class="mt-4 p-3 rounded-xl bg-rose-50 border border-rose-100 text-rose-700 text-sm">
      {{ state.error }}
    </div>

    <div v-if="state.planSteps?.length" class="mt-4 space-y-2">
      <div
        v-for="step in state.planSteps"
        :key="step.index || step.action"
        class="border border-slate-200 rounded-xl px-4 py-2 bg-slate-50"
      >
        <div class="flex items-center justify-between gap-3">
          <div>
            <p class="text-sm font-semibold text-slate-800">
              步骤 {{ step.index || '?' }} · {{ step.action }}
            </p>
            <p class="text-xs text-slate-500 mt-0.5">
              {{ step.comment || '—' }}
            </p>
          </div>
          <span
            class="text-xs font-semibold px-2 py-0.5 rounded-full"
            :class="step.status === 'completed'
              ? 'bg-emerald-50 text-emerald-700'
              : step.status === 'skipped'
                ? 'bg-amber-50 text-amber-700'
                : step.status === 'started'
                  ? 'bg-blue-50 text-blue-700'
                  : 'bg-slate-100 text-slate-600'"
          >
            {{ step.status || 'pending' }}
          </span>
        </div>
        <pre
          v-if="step.details"
          class="mt-2 text-xs text-slate-600 bg-white border border-slate-200 rounded-lg p-2 overflow-auto"
        >{{ JSON.stringify(step.details, null, 2) }}</pre>
      </div>
    </div>

    <div class="grid gap-6 md:grid-cols-2 mt-6">
      <section class="space-y-3">
        <h4 class="text-sm font-semibold text-slate-700 flex items-center gap-2">
          <span class="w-2 h-2 rounded-full bg-amber-400"></span> 过程记录
        </h4>
        <div
          v-if="state.timeline.length === 0"
          class="text-xs text-slate-400 border border-dashed border-slate-200 rounded-xl p-4 text-center"
        >
          暂无事件
        </div>
        <ul v-else class="space-y-2 max-h-72 overflow-auto pr-1">
          <li
            v-for="item in state.timeline"
            :key="item.id"
            class="rounded-xl border border-slate-100 p-3 bg-slate-50"
          >
            <div class="flex items-center justify-between text-xs text-slate-500 mb-1">
              <span>{{ item.title }}</span>
              <span>{{ formatTime(item.time) }}</span>
            </div>
            <p class="text-sm text-slate-700">
              {{ item.message || '-' }}
            </p>
          </li>
        </ul>
      </section>

      <section class="space-y-3">
        <h4 class="text-sm font-semibold text-slate-700 flex items-center gap-2">
          <span class="w-2 h-2 rounded-full bg-emerald-400"></span> 相关切片
        </h4>
        <div
          v-if="!hasResults"
          class="text-xs text-slate-400 border border-dashed border-slate-200 rounded-xl p-4 text-center"
        >
          结果将实时显示在这里
        </div>
        <div v-else class="space-y-4 max-h-80 overflow-auto pr-1">
          <article
            v-for="item in state.results"
            :key="item.id"
            class="border border-slate-100 rounded-xl p-4 bg-white shadow-sm"
          >
            <div class="flex items-start justify-between gap-3">
              <div>
                <p class="text-sm font-semibold text-slate-800">{{ item.file?.filename || '未知文件' }}</p>
                <p class="text-xs text-slate-500 mt-1">
                  步骤：{{ item.step_action || '—' }}
                </p>
              </div>
              <button
                class="text-xs text-blue-600 hover:text-blue-800"
                type="button"
                @click="copyContent(item.content)"
              >
                复制
              </button>
            </div>
            <p class="text-xs text-slate-500 mt-2">
              相关度：{{ item.judge_score?.toFixed(2) ?? '-' }} · {{ item.judge_reason }}
            </p>
            <p v-if="item.refine_reason" class="text-xs text-slate-500 mt-1">
              筛选说明：{{ item.refine_reason }}
            </p>
            <pre class="mt-3 text-sm text-slate-700 bg-slate-50 rounded-lg p-3 max-h-40 overflow-auto whitespace-pre-wrap">{{ item.content }}</pre>
          </article>
        </div>
      </section>
    </div>

    <div v-if="state.debugEvents.length" class="mt-6 border-t border-dashed border-slate-200 pt-4">
      <button
        type="button"
        class="text-xs text-slate-500 hover:text-slate-700 flex items-center gap-1"
        @click="showDebug = !showDebug"
      >
        <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
        </svg>
        {{ showDebug ? '隐藏调试事件' : '显示调试事件' }}
      </button>
      <div v-if="showDebug" class="mt-3 max-h-48 overflow-auto bg-slate-50 rounded-xl p-3 text-xs text-slate-600">
        <div
          v-for="event in state.debugEvents"
          :key="event.id"
          class="mb-2 border-b border-slate-200 pb-2 last:border-none last:pb-0"
        >
          <p class="font-semibold">[{{ event.type }}] {{ formatTime(event.time) }}</p>
          <pre class="whitespace-pre-wrap">{{ JSON.stringify(event.payload, null, 2) }}</pre>
        </div>
      </div>
    </div>
  </div>
</template>
