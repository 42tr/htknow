<script setup>
import { ref, onMounted, computed, watch } from 'vue'
import { api } from '../api.js'
import EntityDetail from './EntityDetail.vue'
import GraphVisualization from './GraphVisualization.vue'

const entities = ref([])
const stats = ref(null)
const loading = ref(false)
const searchQuery = ref('')
const selectedEntityType = ref('')
const selectedEntity = ref(null)
const showEntityDetail = ref(false)
const viewMode = ref('list') // 'list' or 'graph'

// 范围过滤
const scopeType = ref('all') // 'all', 'kb', 'file'
const knowledgeBases = ref([])
const files = ref([])
const selectedKbId = ref(null)
const selectedFileId = ref(null)

// 实体类型映射
const entityTypeMap = {
  'person': { label: '人物', icon: '👤', color: 'blue' },
  'organization': { label: '组织', icon: '🏢', color: 'purple' },
  'location': { label: '地点', icon: '📍', color: 'green' },
  'date': { label: '日期', icon: '📅', color: 'orange' },
  'product': { label: '产品', icon: '📦', color: 'pink' },
  'technology': { label: '技术', icon: '⚡', color: 'cyan' },
  'concept': { label: '概念', icon: '💡', color: 'yellow' },
  'api': { label: 'API', icon: '🔌', color: 'indigo' },
  'document': { label: '文档', icon: '📄', color: 'gray' },
  'chapter': { label: '章节', icon: '📑', color: 'slate' },
  'table': { label: '表格', icon: '📊', color: 'teal' },
  'image': { label: '图片', icon: '🖼️', color: 'rose' },
}

const entityTypes = computed(() => {
  if (!stats.value) return []
  return Object.entries(stats.value.entity_types || {}).map(([type, count]) => ({
    type,
    count,
    ...entityTypeMap[type] || { label: type, icon: '📌', color: 'gray' }
  }))
})

const filteredEntities = computed(() => {
  return entities.value
})

// 当前有效的过滤参数
const currentKbId = computed(() => scopeType.value === 'kb' ? selectedKbId.value : null)
const currentFileId = computed(() => scopeType.value === 'file' ? selectedFileId.value : null)

// 加载知识库列表
const loadKnowledgeBases = async () => {
  try {
    knowledgeBases.value = await api.getKnowledgeBases()
  } catch (error) {
    console.error('加载知识库列表失败:', error)
  }
}

// 加载文件列表
const loadFiles = async () => {
  try {
    files.value = await api.getFiles()
  } catch (error) {
    console.error('加载文件列表失败:', error)
  }
}

const loadStats = async () => {
  try {
    stats.value = await api.getGraphStats(currentKbId.value, currentFileId.value)
  } catch (error) {
    console.error('加载统计失败:', error)
  }
}

const loadEntities = async () => {
  loading.value = true
  try {
    entities.value = await api.searchEntities(
      searchQuery.value || null,
      selectedEntityType.value || null,
      currentKbId.value,
      100,
      currentFileId.value
    )
  } catch (error) {
    console.error('加载实体失败:', error)
  } finally {
    loading.value = false
  }
}

const handleSearch = () => {
  loadStats()
  loadEntities()
}

const handleScopeChange = () => {
  // 切换范围时重置选择
  if (scopeType.value === 'all') {
    selectedKbId.value = null
    selectedFileId.value = null
  } else if (scopeType.value === 'kb') {
    selectedFileId.value = null
    if (knowledgeBases.value.length > 0 && !selectedKbId.value) {
      selectedKbId.value = knowledgeBases.value[0].id
    }
  } else if (scopeType.value === 'file') {
    selectedKbId.value = null
    if (files.value.length > 0 && !selectedFileId.value) {
      selectedFileId.value = files.value[0].id
    }
  }
  loadStats()
  loadEntities()
}

const handleEntityTypeFilter = (type) => {
  selectedEntityType.value = selectedEntityType.value === type ? '' : type
  loadEntities()
}

const handleEntityClick = (entity) => {
  selectedEntity.value = entity
  showEntityDetail.value = true
}

const closeEntityDetail = () => {
  showEntityDetail.value = false
  selectedEntity.value = null
}

const getEntityTypeInfo = (type) => {
  return entityTypeMap[type] || { label: type, icon: '📌', color: 'gray' }
}

const formatDate = (timestamp) => {
  return new Date(timestamp * 1000).toLocaleString('zh-CN')
}

// 监听知识库和文件选择变化
watch(selectedKbId, () => {
  if (scopeType.value === 'kb') {
    loadStats()
    loadEntities()
  }
})

watch(selectedFileId, () => {
  if (scopeType.value === 'file') {
    loadStats()
    loadEntities()
  }
})

onMounted(() => {
  loadKnowledgeBases()
  loadFiles()
  loadStats()
  loadEntities()
})
</script>

<template>
  <div class="space-y-6">
    <!-- 范围选择 -->
    <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
      <h3 class="text-sm font-semibold text-slate-700 mb-4">数据范围</h3>
      <div class="flex flex-wrap items-center gap-4">
        <!-- 范围类型选择 -->
        <div class="flex gap-1 bg-slate-100 p-1 rounded-lg">
          <button
            @click="scopeType = 'all'; handleScopeChange()"
            :class="[
              'px-4 py-2 rounded-lg text-sm font-medium transition-all',
              scopeType === 'all' ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'
            ]"
          >
            🌐 全部
          </button>
          <button
            @click="scopeType = 'kb'; handleScopeChange()"
            :class="[
              'px-4 py-2 rounded-lg text-sm font-medium transition-all',
              scopeType === 'kb' ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'
            ]"
          >
            📚 知识库
          </button>
          <button
            @click="scopeType = 'file'; handleScopeChange()"
            :class="[
              'px-4 py-2 rounded-lg text-sm font-medium transition-all',
              scopeType === 'file' ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'
            ]"
          >
            📄 文件
          </button>
        </div>

        <!-- 知识库选择 -->
        <select
          v-if="scopeType === 'kb'"
          v-model="selectedKbId"
          class="px-4 py-2 border border-slate-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent bg-white min-w-[200px]"
        >
          <option :value="null" disabled>选择知识库...</option>
          <option v-for="kb in knowledgeBases" :key="kb.id" :value="kb.id">
            {{ kb.name }}
          </option>
        </select>

        <!-- 文件选择 -->
        <select
          v-if="scopeType === 'file'"
          v-model="selectedFileId"
          class="px-4 py-2 border border-slate-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent bg-white min-w-[200px]"
        >
          <option :value="null" disabled>选择文件...</option>
          <option v-for="file in files" :key="file.id" :value="file.id">
            {{ file.filename }}
          </option>
        </select>

        <!-- 当前范围提示 -->
        <div class="text-sm text-slate-500">
          <span v-if="scopeType === 'all'">显示所有知识图谱数据</span>
          <span v-else-if="scopeType === 'kb' && selectedKbId">
            已选择知识库: <strong class="text-slate-700">{{ knowledgeBases.find(kb => kb.id === selectedKbId)?.name }}</strong>
          </span>
          <span v-else-if="scopeType === 'file' && selectedFileId">
            已选择文件: <strong class="text-slate-700">{{ files.find(f => f.id === selectedFileId)?.filename }}</strong>
          </span>
        </div>
      </div>
    </div>

    <!-- 统计卡片 -->
    <div v-if="stats" class="grid grid-cols-1 md:grid-cols-4 gap-4">
      <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
        <div class="flex items-center gap-3">
          <div class="w-12 h-12 bg-blue-100 rounded-lg flex items-center justify-center">
            <span class="text-2xl">🔵</span>
          </div>
          <div>
            <p class="text-sm text-slate-500">实体节点</p>
            <p class="text-2xl font-bold text-slate-800">{{ stats.node_count }}</p>
          </div>
        </div>
      </div>

      <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
        <div class="flex items-center gap-3">
          <div class="w-12 h-12 bg-purple-100 rounded-lg flex items-center justify-center">
            <span class="text-2xl">↔️</span>
          </div>
          <div>
            <p class="text-sm text-slate-500">关系边</p>
            <p class="text-2xl font-bold text-slate-800">{{ stats.edge_count }}</p>
          </div>
        </div>
      </div>

      <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
        <div class="flex items-center gap-3">
          <div class="w-12 h-12 bg-green-100 rounded-lg flex items-center justify-center">
            <span class="text-2xl">📊</span>
          </div>
          <div>
            <p class="text-sm text-slate-500">实体类型</p>
            <p class="text-2xl font-bold text-slate-800">{{ Object.keys(stats.entity_types || {}).length }}</p>
          </div>
        </div>
      </div>

      <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
        <div class="flex items-center gap-3">
          <div class="w-12 h-12 bg-orange-100 rounded-lg flex items-center justify-center">
            <span class="text-2xl">🔗</span>
          </div>
          <div>
            <p class="text-sm text-slate-500">关系类型</p>
            <p class="text-2xl font-bold text-slate-800">{{ Object.keys(stats.relation_types || {}).length }}</p>
          </div>
        </div>
      </div>
    </div>

    <!-- 实体类型筛选 -->
    <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
      <h3 class="text-sm font-semibold text-slate-700 mb-4">实体类型</h3>
      <div class="flex flex-wrap gap-2">
        <button
          v-for="entityType in entityTypes"
          :key="entityType.type"
          @click="handleEntityTypeFilter(entityType.type)"
          :class="[
            'px-4 py-2 rounded-lg text-sm font-medium transition-all duration-200 flex items-center gap-2',
            selectedEntityType === entityType.type
              ? `bg-${entityType.color}-100 text-${entityType.color}-700 border-2 border-${entityType.color}-300`
              : 'bg-slate-50 text-slate-600 border-2 border-transparent hover:bg-slate-100'
          ]"
        >
          <span>{{ entityType.icon }}</span>
          <span>{{ entityType.label }}</span>
          <span class="ml-1 px-2 py-0.5 bg-white rounded-full text-xs">{{ entityType.count }}</span>
        </button>
      </div>
    </div>

    <!-- 搜索框 -->
    <div class="bg-white rounded-xl p-6 border border-slate-200 shadow-sm">
      <div class="flex gap-3">
        <input
          v-model="searchQuery"
          @keyup.enter="handleSearch"
          type="text"
          placeholder="搜索实体名称..."
          class="flex-1 px-4 py-3 border border-slate-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
        />
        <button
          @click="handleSearch"
          class="px-6 py-3 bg-blue-500 text-white rounded-lg font-medium hover:bg-blue-600 transition-colors"
        >
          🔍 搜索
        </button>
        <div class="flex gap-1 bg-slate-100 p-1 rounded-lg">
          <button
            @click="viewMode = 'list'"
            :class="[
              'px-4 py-2 rounded-lg text-sm font-medium transition-all',
              viewMode === 'list' ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'
            ]"
          >
            📋 列表
          </button>
          <button
            @click="viewMode = 'graph'"
            :class="[
              'px-4 py-2 rounded-lg text-sm font-medium transition-all',
              viewMode === 'graph' ? 'bg-white text-slate-900 shadow-sm' : 'text-slate-600 hover:text-slate-900'
            ]"
          >
            🕸️ 图形
          </button>
        </div>
      </div>
    </div>

    <!-- 图形可视化视图 -->
    <GraphVisualization
      v-if="viewMode === 'graph'"
      :kb-id="currentKbId"
      :file-id="currentFileId"
      :query="searchQuery || null"
      :entity-type="selectedEntityType || null"
    />

    <!-- 实体列表 -->
    <div v-if="viewMode === 'list'" class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">
      <div class="px-6 py-4 border-b border-slate-200 bg-slate-50">
        <h3 class="font-semibold text-slate-800">
          实体列表
          <span class="text-sm text-slate-500 ml-2">({{ filteredEntities.length }} 个实体)</span>
        </h3>
      </div>

      <div v-if="loading" class="p-8 text-center">
        <div class="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-500"></div>
        <p class="mt-2 text-slate-500">加载中...</p>
      </div>

      <div v-else-if="filteredEntities.length === 0" class="p-8 text-center text-slate-500">
        <span class="text-4xl mb-2 block">🔍</span>
        暂无实体数据
      </div>

      <div v-else class="divide-y divide-slate-100">
        <div
          v-for="entity in filteredEntities"
          :key="entity.id"
          @click="handleEntityClick(entity)"
          class="p-4 hover:bg-slate-50 transition-colors cursor-pointer"
        >
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-3 flex-1">
              <div :class="`w-10 h-10 bg-${getEntityTypeInfo(entity.entity_type).color}-100 rounded-lg flex items-center justify-center`">
                <span class="text-xl">{{ getEntityTypeInfo(entity.entity_type).icon }}</span>
              </div>
              <div class="flex-1">
                <h4 class="font-medium text-slate-800">{{ entity.name }}</h4>
                <div class="flex items-center gap-2 mt-1">
                  <span :class="`px-2 py-0.5 text-xs rounded-full bg-${getEntityTypeInfo(entity.entity_type).color}-100 text-${getEntityTypeInfo(entity.entity_type).color}-700`">
                    {{ getEntityTypeInfo(entity.entity_type).label }}
                  </span>
                  <span class="text-xs text-slate-400">
                    {{ formatDate(entity.created_at) }}
                  </span>
                </div>
              </div>
            </div>
            <div class="text-slate-400">
              →
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 实体详情弹窗 -->
    <EntityDetail
      v-if="showEntityDetail && selectedEntity"
      :entity-id="selectedEntity.id"
      @close="closeEntityDetail"
    />
  </div>
</template>
