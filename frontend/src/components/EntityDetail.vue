<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api.js'

const props = defineProps({
  entityId: {
    type: Number,
    required: true
  }
})

const emit = defineEmits(['close'])

const entityDetail = ref(null)
const loading = ref(false)

const entityTypeMap = {
  'person': { label: '人物', icon: '👤', color: 'blue' },
  'organization': { label: '组织', icon: '🏢', color: 'purple' },
  'location': { label: '地点', icon: '📍', color: 'green' },
  'date': { label: '日期', icon: '📅', color: 'orange' },
  'product': { label: '产品', icon: '📦', color: 'pink' },
  'technology': { label: '技术', icon: '⚡', color: 'cyan' },
  'concept': { label: '概念', icon: '💡', color: 'yellow' },
  'api': { label: 'API', icon: '🔌', color: 'indigo' },
}

const relationTypeMap = {
  'cooccurs': { label: '共现', icon: '🔗', color: 'blue' },
  'isa': { label: '是一种', icon: '📌', color: 'purple' },
  'partof': { label: '部分', icon: '🧩', color: 'green' },
  'hasproperty': { label: '具有属性', icon: '⚙️', color: 'orange' },
  'dependson': { label: '依赖于', icon: '🔄', color: 'pink' },
  'relatedto': { label: '相关', icon: '↔️', color: 'cyan' },
  'contains': { label: '包含', icon: '📦', color: 'indigo' },
  'mentionedin': { label: '提及于', icon: '📝', color: 'yellow' },
}

const getEntityTypeInfo = (type) => {
  return entityTypeMap[type] || { label: type, icon: '📌', color: 'gray' }
}

const getRelationTypeInfo = (type) => {
  return relationTypeMap[type] || { label: type, icon: '🔗', color: 'gray' }
}

const loadEntityDetail = async () => {
  loading.value = true
  try {
    entityDetail.value = await api.getEntity(props.entityId)
  } catch (error) {
    console.error('加载实体详情失败:', error)
  } finally {
    loading.value = false
  }
}

const handleClose = () => {
  emit('close')
}

onMounted(() => {
  loadEntityDetail()
})
</script>

<template>
  <div class="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4" @click.self="handleClose">
    <div class="bg-white rounded-2xl max-w-4xl w-full max-h-[90vh] overflow-hidden shadow-2xl">
      <!-- 头部 -->
      <div class="px-6 py-4 border-b border-slate-200 flex items-center justify-between bg-gradient-to-r from-blue-50 to-indigo-50">
        <div class="flex items-center gap-3">
          <div v-if="entityDetail" :class="`w-12 h-12 bg-${getEntityTypeInfo(entityDetail.entity.entity_type).color}-100 rounded-xl flex items-center justify-center`">
            <span class="text-2xl">{{ getEntityTypeInfo(entityDetail.entity.entity_type).icon }}</span>
          </div>
          <div>
            <h2 class="text-xl font-bold text-slate-800">
              {{ entityDetail?.entity.name || '加载中...' }}
            </h2>
            <p v-if="entityDetail" class="text-sm text-slate-500">
              {{ getEntityTypeInfo(entityDetail.entity.entity_type).label }}
            </p>
          </div>
        </div>
        <button
          @click="handleClose"
          class="w-8 h-8 rounded-lg hover:bg-slate-200 transition-colors flex items-center justify-center"
        >
          ✕
        </button>
      </div>

      <!-- 内容 -->
      <div class="overflow-y-auto max-h-[calc(90vh-80px)]">
        <div v-if="loading" class="p-8 text-center">
          <div class="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-500"></div>
          <p class="mt-2 text-slate-500">加载中...</p>
        </div>

        <div v-else-if="entityDetail" class="p-6 space-y-6">
          <!-- 基本信息 -->
          <div class="bg-slate-50 rounded-xl p-4">
            <h3 class="text-sm font-semibold text-slate-700 mb-3">基本信息</h3>
            <div class="grid grid-cols-2 gap-4">
              <div>
                <p class="text-xs text-slate-500">实体ID</p>
                <p class="text-sm font-medium text-slate-800">{{ entityDetail.entity.id }}</p>
              </div>
              <div>
                <p class="text-xs text-slate-500">实体类型</p>
                <p class="text-sm font-medium text-slate-800">{{ getEntityTypeInfo(entityDetail.entity.entity_type).label }}</p>
              </div>
              <div v-if="entityDetail.entity.file_id">
                <p class="text-xs text-slate-500">来源文件ID</p>
                <p class="text-sm font-medium text-slate-800">{{ entityDetail.entity.file_id }}</p>
              </div>
              <div v-if="entityDetail.entity.kb_id">
                <p class="text-xs text-slate-500">知识库ID</p>
                <p class="text-sm font-medium text-slate-800">{{ entityDetail.entity.kb_id }}</p>
              </div>
            </div>
          </div>

          <!-- 关联实体 -->
          <div v-if="entityDetail.neighbors && entityDetail.neighbors.length > 0">
            <h3 class="text-sm font-semibold text-slate-700 mb-3 flex items-center gap-2">
              <span>🔗</span>
              <span>关联实体 ({{ entityDetail.neighbors.length }})</span>
            </h3>
            <div class="space-y-2">
              <div
                v-for="neighbor in entityDetail.neighbors"
                :key="neighbor.entity.id"
                class="bg-white border border-slate-200 rounded-lg p-4 hover:border-blue-300 transition-colors"
              >
                <div class="flex items-center gap-3">
                  <div :class="`w-10 h-10 bg-${getEntityTypeInfo(neighbor.entity.entity_type).color}-100 rounded-lg flex items-center justify-center flex-shrink-0`">
                    <span class="text-lg">{{ getEntityTypeInfo(neighbor.entity.entity_type).icon }}</span>
                  </div>
                  <div class="flex-1">
                    <div class="flex items-center gap-2">
                      <h4 class="font-medium text-slate-800">{{ neighbor.entity.name }}</h4>
                      <span :class="`px-2 py-0.5 text-xs rounded-full bg-${getEntityTypeInfo(neighbor.entity.entity_type).color}-100 text-${getEntityTypeInfo(neighbor.entity.entity_type).color}-700`">
                        {{ getEntityTypeInfo(neighbor.entity.entity_type).label }}
                      </span>
                    </div>
                    <div class="flex items-center gap-2 mt-1">
                      <span :class="`px-2 py-0.5 text-xs rounded-full bg-${getRelationTypeInfo(neighbor.relation_type).color}-100 text-${getRelationTypeInfo(neighbor.relation_type).color}-700 flex items-center gap-1`">
                        <span>{{ getRelationTypeInfo(neighbor.relation_type).icon }}</span>
                        <span>{{ getRelationTypeInfo(neighbor.relation_type).label }}</span>
                      </span>
                      <span class="text-xs text-slate-400">
                        {{ neighbor.direction === 'outgoing' ? '→ 出边' : '← 入边' }}
                      </span>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>

          <!-- 文档提及 -->
          <div v-if="entityDetail.mentions && entityDetail.mentions.length > 0">
            <h3 class="text-sm font-semibold text-slate-700 mb-3 flex items-center gap-2">
              <span>📝</span>
              <span>文档提及 ({{ entityDetail.mentions.length }})</span>
            </h3>
            <div class="space-y-2">
              <div
                v-for="mention in entityDetail.mentions"
                :key="mention.slice_id"
                class="bg-slate-50 rounded-lg p-4 border border-slate-200"
              >
                <div class="flex items-start gap-2 mb-2">
                  <span class="text-xs font-medium text-blue-600">📄 {{ mention.filename }}</span>
                  <span class="text-xs text-slate-400">切片ID: {{ mention.slice_id }}</span>
                </div>
                <p class="text-sm text-slate-700 leading-relaxed">
                  {{ mention.context }}
                </p>
              </div>
            </div>
          </div>

          <!-- 空状态 -->
          <div v-if="(!entityDetail.neighbors || entityDetail.neighbors.length === 0) && (!entityDetail.mentions || entityDetail.mentions.length === 0)" class="text-center py-8 text-slate-400">
            <span class="text-4xl block mb-2">📭</span>
            暂无关联信息
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
