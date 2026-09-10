<script setup>
import { ref, computed, onMounted } from 'vue'
import { vDialog } from '../dialog'
import { api } from '../api.js'
import EntityDetail from './EntityDetail.vue'
import GraphVisualization from './GraphVisualization.vue'

const props = defineProps({
  kbId: { type: [Number, String], default: null },
  fileId: { type: [Number, String], default: null },
  title: { type: String, default: '知识图谱' },
  subtitle: { type: String, default: '' },
  maxNodes: { type: Number, default: 100 },
})
const emit = defineEmits(['close'])

const entityTypeMap = {
  person: { label: '人物' },
  organization: { label: '组织' },
  location: { label: '地点' },
  date: { label: '日期' },
  product: { label: '产品' },
  technology: { label: '技术' },
  concept: { label: '概念' },
  api: { label: 'API' },
  document: { label: '文档' },
  chapter: { label: '章节' },
  table: { label: '表格' },
  image: { label: '图片' },
}

const entities = ref([])
const stats = ref(null)
const loading = ref(false)
const error = ref('')
const searchQuery = ref('')
const selectedEntityType = ref('')
const selectedEntity = ref(null)
const showEntityDetail = ref(false)
const viewMode = ref('graph')

const entityTypes = computed(() =>
  Object.entries(stats.value?.entity_types || {}).map(([type, count]) => ({
    type,
    count,
    label: entityTypeMap[type]?.label || type,
  })),
)
const summary = computed(() => [
  { label: '实体节点', value: stats.value?.node_count ?? 0 },
  { label: '关系边', value: stats.value?.edge_count ?? 0 },
  {
    label: '实体类型',
    value: Object.keys(stats.value?.entity_types || {}).length,
  },
  {
    label: '关系类型',
    value: Object.keys(stats.value?.relation_types || {}).length,
  },
])
const buildStatus = computed(
  () =>
    ({
      running: '图谱构建：处理中',
      completed: '图谱构建：已完成',
      failed: '图谱构建：失败，可重新解析文件后重试',
    })[stats.value?.build?.status] || '',
)
const entityLabel = (type) => entityTypeMap[type]?.label || type

const loadStats = async () => {
  try {
    stats.value = await api.getGraphStats(props.kbId, props.fileId)
  } catch (e) {
    error.value = e?.message || '加载图谱统计失败'
  }
}

const loadEntities = async () => {
  loading.value = true
  try {
    entities.value = await api.searchEntities(
      searchQuery.value || null,
      selectedEntityType.value || null,
      props.kbId,
      100,
      props.fileId,
    )
  } catch (e) {
    error.value = e?.message || '加载实体失败'
  } finally {
    loading.value = false
  }
}

const handleEntityTypeFilter = (type) => {
  selectedEntityType.value = selectedEntityType.value === type ? '' : type
  loadEntities()
}

const openEntity = (entity) => {
  selectedEntity.value = entity
  showEntityDetail.value = true
}

const closeEntityDetail = () => {
  showEntityDetail.value = false
  selectedEntity.value = null
}

const formatDate = (timestamp) =>
  new Date(timestamp * 1000).toLocaleString('zh-CN')

onMounted(() => {
  loadStats()
  loadEntities()
})
</script>

<template>
  <Teleport to="body">
    <div
      class="upload-overlay"
      @click.self="emit('close')"
      @keydown.esc="emit('close')"
    >
      <section
        v-dialog
        class="graph-dialog"
        role="dialog"
        aria-modal="true"
        :aria-label="title"
      >
        <div class="panel-heading">
          <div>
            <h2>{{ title }}</h2>
            <p>{{ subtitle || '查看文档中抽取的实体与关系。' }}</p>
          </div>
          <button
            class="plain-button"
            aria-label="关闭知识图谱"
            @click="emit('close')"
          >
            ✕
          </button>
        </div>
        <div class="graph-dialog-body">
          <div class="graph-summary">
            <span v-for="item in summary" :key="item.label"
              >{{ item.label }} <b>{{ item.value }}</b></span
            >
            <span v-if="buildStatus" class="graph-build-status">{{
              buildStatus
            }}</span>
            <span class="graph-view-toggle">
              <button
                :class="{ active: viewMode === 'graph' }"
                @click="viewMode = 'graph'"
              >
                图形
              </button>
              <button
                :class="{ active: viewMode === 'list' }"
                @click="viewMode = 'list'"
              >
                实体列表
              </button>
            </span>
          </div>
          <p v-if="error" class="inline-error">{{ error }}</p>

          <GraphVisualization
            v-if="viewMode === 'graph'"
            :kb-id="kbId"
            :file-id="fileId"
            :max-nodes="maxNodes"
          />

          <template v-else>
            <div class="graph-list-controls">
              <input
                v-model="searchQuery"
                type="search"
                placeholder="搜索实体名称..."
                @keyup.enter="loadEntities"
              />
              <button class="secondary-button" @click="loadEntities">
                搜索
              </button>
            </div>
            <div v-if="entityTypes.length" class="graph-entity-types">
              <button
                v-for="type in entityTypes"
                :key="type.type"
                class="graph-chip"
                :class="{ active: selectedEntityType === type.type }"
                @click="handleEntityTypeFilter(type.type)"
              >
                {{ type.label }} <b>{{ type.count }}</b>
              </button>
            </div>
            <p v-if="loading" class="graph-empty">正在加载实体...</p>
            <p v-else-if="!entities.length" class="graph-empty">
              暂无实体数据，解析文档后可在此查看。
            </p>
            <ul v-else class="graph-entity-list">
              <li v-for="entity in entities" :key="entity.id">
                <button class="graph-entity-row" @click="openEntity(entity)">
                  <span class="graph-entity-name">{{ entity.name }}</span>
                  <span class="graph-entity-type">{{
                    entityLabel(entity.entity_type)
                  }}</span>
                  <span class="graph-entity-date">{{
                    formatDate(entity.created_at)
                  }}</span>
                  <span class="graph-entity-arrow" aria-hidden="true">→</span>
                </button>
              </li>
            </ul>
          </template>
        </div>
      </section>
    </div>
    <EntityDetail
      v-if="showEntityDetail && selectedEntity"
      :entity-id="selectedEntity.id"
      @close="closeEntityDetail"
    />
  </Teleport>
</template>
