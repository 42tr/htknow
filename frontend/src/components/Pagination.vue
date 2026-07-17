<script setup>
import { computed } from 'vue'

const props = defineProps({
  page: {
    type: Number,
    required: true,
  },
  size: {
    type: Number,
    required: true,
  },
  total: {
    type: Number,
    required: true,
  },
  sizes: {
    type: Array,
    default: () => [10, 20, 50],
  },
})

const emit = defineEmits(['update:page', 'update:size', 'change'])

const totalPages = computed(() => Math.max(1, Math.ceil(props.total / props.size)))

const visiblePages = computed(() => {
  const current = props.page
  const last = totalPages.value
  if (last <= 7) {
    return Array.from({ length: last }, (_, i) => i + 1)
  }
  if (current <= 4) {
    return [1, 2, 3, 4, 5, '...', last]
  }
  if (current >= last - 3) {
    return [1, '...', last - 4, last - 3, last - 2, last - 1, last]
  }
  return [1, '...', current - 1, current, current + 1, '...', last]
})

function setPage(p) {
  if (p === '...' || p < 1 || p > totalPages.value || p === props.page) return
  emit('update:page', p)
  emit('change')
}

function prev() {
  if (props.page > 1) setPage(props.page - 1)
}

function next() {
  if (props.page < totalPages.value) setPage(props.page + 1)
}

function onSizeChange(event) {
  emit('update:size', Number(event.target.value))
  emit('update:page', 1)
  emit('change')
}
</script>

<template>
  <div v-if="total > 0" class="flex flex-col sm:flex-row items-center justify-between gap-3 mt-6">
    <div class="text-sm text-slate-500">
      共 {{ total }} 条，第 {{ page }} / {{ totalPages }} 页
    </div>
    <div class="flex items-center gap-2">
      <select
        :value="size"
        @change="onSizeChange"
        class="px-2 py-1.5 text-sm border border-slate-200 rounded-md bg-white text-slate-700 focus:outline-none focus:ring-2 focus:ring-blue-500"
      >
        <option v-for="s in sizes" :key="s" :value="s">{{ s }} 条/页</option>
      </select>

      <button
        type="button"
        :disabled="page <= 1"
        @click="prev"
        class="px-3 py-1.5 text-sm border border-slate-200 rounded-md bg-white text-slate-700 hover:bg-slate-50 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        上一页
      </button>

      <button
        v-for="p in visiblePages"
        :key="String(p) + '-' + page"
        type="button"
        :disabled="p === '...'"
        @click="() => setPage(p)"
        :class="[
          'px-3 py-1.5 text-sm border rounded-md min-w-[2.5rem]',
          p === page
            ? 'bg-blue-600 border-blue-600 text-white'
            : 'bg-white border-slate-200 text-slate-700 hover:bg-slate-50',
          p === '...' && 'cursor-default opacity-60'
        ]"
      >
        {{ p }}
      </button>

      <button
        type="button"
        :disabled="page >= totalPages"
        @click="next"
        class="px-3 py-1.5 text-sm border border-slate-200 rounded-md bg-white text-slate-700 hover:bg-slate-50 disabled:opacity-50 disabled:cursor-not-allowed"
      >
        下一页
      </button>
    </div>
  </div>
</template>
