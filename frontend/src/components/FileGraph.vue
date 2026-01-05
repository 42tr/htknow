<script setup>
import { ref, onMounted, computed } from 'vue'
import { api } from '../api.js'
import GraphVisualization from './GraphVisualization.vue'
import EntityDetail from './EntityDetail.vue'

const props = defineProps({
  file: {
    type: Object,
    required: true
  }
})

const emit = defineEmits(['close'])

const stats = ref(null)
const loading = ref(true)
const selectedEntity = ref(null)
const showEntityDetail = ref(false)

const loadStats = async () => {
  loading.value = true
  try {
    stats.value = await api.getGraphStats(null, props.file.id)
  } catch (error) {
    console.error('加载图谱统计失败:', error)
  } finally {
    loading.value = false
  }
}

const handleEntityClick = (entity) => {
  selectedEntity.value = entity
  showEntityDetail.value = true
}

const closeEntityDetail = () => {
  showEntityDetail.value = false
  selectedEntity.value = null
}

onMounted(() => {
  loadStats()
})
</script>

<template>
  <Teleport to="body">
    <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
      <!-- Backdrop -->
      <div @click="emit('close')" class="absolute inset-0 bg-black/50 backdrop-blur-sm"></div>

      <!-- Modal -->
      <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-6xl max-h-[90vh] overflow-hidden flex flex-col">
        <!-- Header -->
        <div class="px-6 py-4 border-b border-slate-200 flex items-center justify-between flex-shrink-0">
          <div class="flex items-center gap-3">
            <div class="w-10 h-10 bg-gradient-to-br from-purple-100 to-indigo-100 rounded-lg flex items-center justify-center">
              <span class="text-xl">🕸️</span>
            </div>
            <div>
              <h2 class="text-lg font-semibold text-slate-800">文件知识图谱</h2>
              <p class="text-sm text-slate-500">{{ file.filename }}</p>
            </div>
          </div>
          <button
            @click="emit('close')"
            class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-all"
          >
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <!-- Stats -->
        <div v-if="stats" class="px-6 py-3 border-b border-slate-100 bg-slate-50 flex-shrink-0">
          <div class="flex items-center gap-6 text-sm">
            <div class="flex items-center gap-2">
              <span class="text-blue-500">🔵</span>
              <span class="text-slate-600">实体节点:</span>
              <span class="font-semibold text-slate-800">{{ stats.node_count }}</span>
            </div>
            <div class="flex items-center gap-2">
              <span class="text-purple-500">↔️</span>
              <span class="text-slate-600">关系边:</span>
              <span class="font-semibold text-slate-800">{{ stats.edge_count }}</span>
            </div>
            <div class="flex items-center gap-2">
              <span class="text-green-500">📊</span>
              <span class="text-slate-600">实体类型:</span>
              <span class="font-semibold text-slate-800">{{ Object.keys(stats.entity_types || {}).length }}</span>
            </div>
            <div class="flex items-center gap-2">
              <span class="text-orange-500">🔗</span>
              <span class="text-slate-600">关系类型:</span>
              <span class="font-semibold text-slate-800">{{ Object.keys(stats.relation_types || {}).length }}</span>
            </div>
          </div>
        </div>

        <!-- Graph Visualization -->
        <div class="flex-1 overflow-auto p-4">
          <div v-if="stats && stats.node_count === 0" class="flex items-center justify-center h-96">
            <div class="text-center text-slate-400">
              <span class="text-4xl block mb-2">🕸️</span>
              <p>此文件暂无知识图谱数据</p>
              <p class="text-sm mt-1">文件处理完成后将自动生成知识图谱</p>
            </div>
          </div>
          <GraphVisualization v-else :file-id="file.id" :max-nodes="100" />
        </div>
      </div>
    </div>

    <!-- Entity Detail -->
    <EntityDetail
      v-if="showEntityDetail && selectedEntity"
      :entity-id="selectedEntity.id"
      @close="closeEntityDetail"
    />
  </Teleport>
</template>
