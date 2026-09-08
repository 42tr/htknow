<script setup>
import { computed, ref, watch } from 'vue'
import { api } from '../api'
import ExportRecordPanel from './ExportRecordPanel.vue'
import KnowledgeBaseExportTreeNode from './KnowledgeBaseExportTreeNode.vue'

const props = defineProps({
  show: Boolean,
  records: { type: Array, default: () => [] },
})

const emit = defineEmits(['close', 'exported', 'clear-records'])
const tree = ref([])
const selected = ref(new Map())
const loading = ref(false)
const exporting = ref(false)
const error = ref('')
const includeChildren = ref(false)

const selectedCount = computed(() => selected.value.size)
const selectedIds = computed(() => Array.from(selected.value.keys()))
const selectedNames = computed(() => Array.from(selected.value.values()).map((kb) => kb.name))

const loadBranch = async (parentId = null) => {
  const result = await api.getKnowledgeBases(parentId)
  const nodes = result.items || []
  return Promise.all(nodes.map(async (kb) => ({
    ...kb,
    children: await loadBranch(kb.id),
  })))
}

const loadTree = async () => {
  loading.value = true
  error.value = ''
  try {
    tree.value = await loadBranch()
  } catch (err) {
    error.value = err?.message || '加载知识库目录失败'
  } finally {
    loading.value = false
  }
}

const toggle = (kb) => {
  const next = new Map(selected.value)
  if (next.has(kb.id)) next.delete(kb.id)
  else next.set(kb.id, { id: kb.id, name: kb.name })
  selected.value = next
}

const clearSelection = () => {
  selected.value = new Map()
}

const handleExport = async () => {
  if (selectedCount.value === 0 || exporting.value) return
  exporting.value = true
  error.value = ''
  try {
    const result = await api.exportKnowledgeBases(selectedIds.value, includeChildren.value)
    emit('exported', result)
    clearSelection()
  } catch (err) {
    error.value = err?.message || '导出知识库失败'
  } finally {
    exporting.value = false
  }
}

watch(() => props.show, (show) => {
  if (show && tree.value.length === 0 && !loading.value) loadTree()
})
</script>

<template>
  <Teleport to="body">
    <div v-if="show" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35 p-4 backdrop-blur-sm" @click.self="emit('close')">
      <section class="flex max-h-[min(760px,calc(100vh-32px))] w-full max-w-4xl flex-col overflow-hidden rounded-2xl border border-slate-200 bg-white shadow-2xl">
        <header class="flex items-center justify-between border-b border-slate-200 px-5 py-4 sm:px-6">
          <div>
            <h3 class="text-lg font-semibold text-slate-800">导出知识库</h3>
            <p class="mt-1 text-sm text-slate-500">从目录树中选择需要导出的知识库</p>
          </div>
          <button type="button" class="flex h-8 w-8 items-center justify-center rounded-lg text-slate-400 hover:bg-slate-100 hover:text-slate-700" aria-label="关闭" @click="emit('close')">
            <svg class="h-5 w-5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="1.8" d="m6 6 12 12M18 6 6 18" /></svg>
          </button>
        </header>

        <div class="grid min-h-0 flex-1 lg:grid-cols-[minmax(0,1.1fr)_minmax(280px,0.9fr)]">
          <div class="min-h-0 border-b border-slate-200 p-5 lg:border-b-0 lg:border-r sm:p-6">
            <div class="mb-4 flex items-center justify-between gap-3">
              <p class="text-xs font-semibold tracking-wide text-slate-500">知识库目录</p>
              <button type="button" class="text-xs text-slate-500 hover:text-slate-800" @click="loadTree">刷新目录</button>
            </div>
            <div v-if="loading" class="flex h-48 items-center justify-center text-sm text-slate-400">正在加载目录...</div>
            <p v-else-if="error && tree.length === 0" class="rounded-lg bg-red-50 px-3 py-2 text-sm text-red-600">{{ error }}</p>
            <div v-else class="max-h-[320px] space-y-1 overflow-y-auto pr-2 lg:max-h-[480px]">
              <KnowledgeBaseExportTreeNode v-for="node in tree" :key="node.id" :node="node" :selected="selected" @toggle="toggle" />
              <p v-if="tree.length === 0" class="py-10 text-center text-sm text-slate-400">暂无可导出的知识库</p>
            </div>
          </div>

          <aside class="min-h-0 bg-slate-50/60 p-5 sm:p-6">
            <p class="text-xs font-semibold tracking-wide text-slate-500">导出选项</p>
            <div class="mt-4 rounded-xl border border-slate-200 bg-white p-4">
              <div class="flex items-start gap-3">
                <input id="include-children" v-model="includeChildren" type="checkbox" class="mt-0.5 h-4 w-4 rounded border-slate-300" />
                <label for="include-children" class="cursor-pointer text-sm text-slate-700">
                  <span class="font-medium">包含子知识库</span>
                  <span class="mt-1 block text-xs leading-5 text-slate-400">导出所选知识库下的全部层级内容</span>
                </label>
              </div>
            </div>
            <div class="mt-4 flex items-center justify-between text-sm">
              <span class="text-slate-500">已选择</span>
              <span class="font-semibold text-slate-800">{{ selectedCount }} 个知识库</span>
            </div>
            <p v-if="selectedCount" class="mt-2 line-clamp-2 text-xs leading-5 text-slate-500">{{ selectedNames.join('、') }}</p>
            <button type="button" class="mt-5 w-full rounded-xl bg-slate-900 px-4 py-2.5 text-sm font-medium text-white transition hover:bg-slate-700 disabled:cursor-not-allowed disabled:bg-slate-200 disabled:text-slate-400" :disabled="selectedCount === 0 || exporting" @click="handleExport">
              {{ exporting ? '导出中...' : `导出 ${selectedCount} 个知识库` }}
            </button>
            <button v-if="selectedCount" type="button" class="mt-2 w-full text-xs text-slate-500 hover:text-slate-800" @click="clearSelection">清空选择</button>
            <p v-if="error && tree.length" class="mt-4 rounded-lg bg-red-50 px-3 py-2 text-xs text-red-600">{{ error }}</p>
          </aside>
        </div>

        <div class="border-t border-slate-200 px-5 py-4 sm:px-6">
          <ExportRecordPanel :records="records" @clear="emit('clear-records')" />
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
:deep(.mt-8) { margin-top: 0; }
</style>
