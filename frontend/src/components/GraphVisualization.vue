<script setup>
import { ref, onMounted, onBeforeUnmount, computed, watch } from 'vue'
import { api } from '../api.js'

const props = defineProps({
  kbId: {
    type: Number,
    default: null
  },
  maxNodes: {
    type: Number,
    default: 50
  }
})

const canvas = ref(null)
const container = ref(null)
let ctx = null
let animationFrame = null

const nodes = ref([])
const edges = ref([])
const loading = ref(false)
const selectedNode = ref(null)
const hoveredNode = ref(null)
const draggedNode = ref(null)
const isDragging = ref(false)

// 缩放和平移
const scale = ref(1)
const offsetX = ref(0)
const offsetY = ref(0)
const isPanning = ref(false)
const lastMouseX = ref(0)
const lastMouseY = ref(0)

// 搜索和筛选
const searchQuery = ref('')
const selectedEntityTypes = ref([])
const selectedRelationTypes = ref([])

// 全部节点和边（未筛选）
const allNodes = ref([])
const allEdges = ref([])

// 物理引擎参数
const physics = {
  centerForce: 0.005,      // 降低中心力，减少抖动
  repelForce: 5000,        // 增加排斥力，避免节点重叠
  linkForce: 0.03,         // 降低拉力，减少振荡
  damping: 0.92,           // 增加阻尼，更快稳定
  maxSpeed: 3,             // 降低最大速度，减少抖动
  minDistance: 100,        // 最小距离，避免节点靠太近
}

// 颜色配置
const colors = {
  'person': '#3b82f6',
  'organization': '#a855f7',
  'location': '#10b981',
  'date': '#f97316',
  'product': '#ec4899',
  'technology': '#06b6d4',
  'concept': '#eab308',
  'api': '#6366f1',
  'default': '#64748b'
}

const relationColors = {
  'cooccurs': '#93c5fd',      // 浅蓝色 - 共现
  'isa': '#c4b5fd',           // 浅紫色 - 是/分类
  'partof': '#86efac',        // 浅绿色 - 部分
  'relatedto': '#67e8f9',     // 青色 - 相关
  'uses': '#fcd34d',          // 黄色 - 使用
  'works_for': '#f9a8d4',     // 粉色 - 任职
  'manages': '#fdba74',       // 橙色 - 管理
  'depends_on': '#a78bfa',    // 紫色 - 依赖
  'creates': '#6ee7b7',       // 绿松石 - 创建
  'implements': '#7dd3fc',    // 天蓝色 - 实现
  'extends': '#c084fc',       // 紫红色 - 扩展
  'located_in': '#fca5a5',    // 浅红色 - 位于
  'default': '#cbd5e1'        // 灰色 - 其他
}

// 初始化画布
const initCanvas = () => {
  if (!canvas.value || !container.value) return
  
  const rect = container.value.getBoundingClientRect()
  canvas.value.width = rect.width
  canvas.value.height = rect.height
  ctx = canvas.value.getContext('2d')
}

// 加载图谱数据
const loadGraphData = async () => {
  loading.value = true
  try {
    // 获取实体
    const entities = await api.searchEntities(null, null, props.kbId, props.maxNodes)
    
    // 将实体转换为节点
    const width = canvas.value.width
    const height = canvas.value.height
    
    allNodes.value = entities.map((entity, i) => ({
      id: entity.id,
      name: entity.name,
      type: entity.entity_type,
      x: Math.random() * width,
      y: Math.random() * height,
      vx: 0,
      vy: 0,
      radius: 8,
      entity: entity
    }))
    
    // 获取每个实体的邻居来构建边
    allEdges.value = []
    const addedEdges = new Set()
    
    // 加载所有节点的关系
    for (const node of allNodes.value) {
      try {
        const detail = await api.getEntity(node.id)
        
        for (const neighbor of detail.neighbors) {
          const targetNode = allNodes.value.find(n => n.id === neighbor.entity.id)
          if (targetNode) {
            const edgeKey = `${Math.min(node.id, targetNode.id)}-${Math.max(node.id, targetNode.id)}`
            if (!addedEdges.has(edgeKey)) {
              allEdges.value.push({
                source: node,
                target: targetNode,
                type: neighbor.relation_type
              })
              addedEdges.add(edgeKey)
            }
          }
        }
      } catch (e) {
        console.warn('Failed to load neighbor for node', node.id)
      }
    }
    
    // 应用筛选
    applyFilters()
    
  } catch (error) {
    console.error('加载图谱数据失败:', error)
  } finally {
    loading.value = false
  }
}

// 应用筛选
const applyFilters = () => {
  // 筛选节点
  let filteredNodes = allNodes.value
  
  // 按实体类型筛选
  if (selectedEntityTypes.value.length > 0) {
    filteredNodes = filteredNodes.filter(node => 
      selectedEntityTypes.value.includes(node.type)
    )
  }
  
  // 按搜索查询筛选
  if (searchQuery.value.trim()) {
    const query = searchQuery.value.toLowerCase().trim()
    filteredNodes = filteredNodes.filter(node =>
      node.name.toLowerCase().includes(query)
    )
  }
  
  nodes.value = filteredNodes
  
  // 筛选边（只保留两端节点都存在的边）
  const nodeIds = new Set(nodes.value.map(n => n.id))
  let filteredEdges = allEdges.value.filter(edge =>
    nodeIds.has(edge.source.id) && nodeIds.has(edge.target.id)
  )
  
  // 按关系类型筛选
  if (selectedRelationTypes.value.length > 0) {
    filteredEdges = filteredEdges.filter(edge =>
      selectedRelationTypes.value.includes(edge.type)
    )
  }
  
  edges.value = filteredEdges
}

// 获取所有实体类型
const availableEntityTypes = computed(() => {
  const types = new Set(allNodes.value.map(n => n.type))
  return Array.from(types).map(type => ({
    type,
    ...entityTypeMap[type] || { label: type, icon: '📌' },
    count: allNodes.value.filter(n => n.type === type).length
  }))
})

// 获取所有关系类型
const availableRelationTypes = computed(() => {
  const types = new Set(allEdges.value.map(e => e.type))
  return Array.from(types).map(type => ({
    type,
    label: type,
    count: allEdges.value.filter(e => e.type === type).length
  }))
})

// 切换实体类型筛选
const toggleEntityType = (type) => {
  const index = selectedEntityTypes.value.indexOf(type)
  if (index === -1) {
    selectedEntityTypes.value.push(type)
  } else {
    selectedEntityTypes.value.splice(index, 1)
  }
  applyFilters()
}

// 切换关系类型筛选
const toggleRelationType = (type) => {
  const index = selectedRelationTypes.value.indexOf(type)
  if (index === -1) {
    selectedRelationTypes.value.push(type)
  } else {
    selectedRelationTypes.value.splice(index, 1)
  }
  applyFilters()
}

// 清除所有筛选
const clearFilters = () => {
  searchQuery.value = ''
  selectedEntityTypes.value = []
  selectedRelationTypes.value = []
  applyFilters()
}

// 监听搜索查询变化
watch(searchQuery, () => {
  applyFilters()
})

// 物理模拟更新
const updatePhysics = () => {
  const width = canvas.value.width
  const height = canvas.value.height
  const centerX = width / 2
  const centerY = height / 2
  
  // 应用力
  for (const node of nodes.value) {
    // 如果节点被拖拽，跳过物理模拟
    if (draggedNode.value && draggedNode.value.id === node.id) {
      node.vx = 0
      node.vy = 0
      continue
    }
    
    // 向中心的力
    const dx = centerX - node.x
    const dy = centerY - node.y
    node.vx += dx * physics.centerForce
    node.vy += dy * physics.centerForce
    
    // 节点间排斥力
    for (const other of nodes.value) {
      if (node === other) continue
      const dx = node.x - other.x
      const dy = node.y - other.y
      const distSq = dx * dx + dy * dy + 1
      
      // 应用最小距离约束
      const minDistSq = physics.minDistance * physics.minDistance
      if (distSq < minDistSq) {
        const force = physics.repelForce / minDistSq
        node.vx += dx * force
        node.vy += dy * force
      } else {
        const force = physics.repelForce / distSq
        node.vx += dx * force
        node.vy += dy * force
      }
    }
  }
  
  // 边的拉力
  for (const edge of edges.value) {
    const dx = edge.target.x - edge.source.x
    const dy = edge.target.y - edge.source.y
    edge.source.vx += dx * physics.linkForce
    edge.source.vy += dy * physics.linkForce
    edge.target.vx -= dx * physics.linkForce
    edge.target.vy -= dy * physics.linkForce
  }
  
  // 更新位置
  for (const node of nodes.value) {
    // 跳过被拖拽的节点
    if (draggedNode.value && draggedNode.value.id === node.id) {
      continue
    }
    
    node.vx *= physics.damping
    node.vy *= physics.damping
    
    // 限制最大速度
    const speed = Math.sqrt(node.vx * node.vx + node.vy * node.vy)
    if (speed > physics.maxSpeed) {
      node.vx = (node.vx / speed) * physics.maxSpeed
      node.vy = (node.vy / speed) * physics.maxSpeed
    }
    
    node.x += node.vx
    node.y += node.vy
    
    // 边界反弹
    if (node.x < 50) { node.x = 50; node.vx *= -0.5 }
    if (node.x > width - 50) { node.x = width - 50; node.vx *= -0.5 }
    if (node.y < 50) { node.y = 50; node.vy *= -0.5 }
    if (node.y > height - 50) { node.y = height - 50; node.vy *= -0.5 }
  }
}

// 渲染
const render = () => {
  if (!ctx || !canvas.value) return
  
  const width = canvas.value.width
  const height = canvas.value.height
  
  // 清空画布
  ctx.clearRect(0, 0, width, height)
  
  // 绘制背景
  ctx.fillStyle = '#f8fafc'
  ctx.fillRect(0, 0, width, height)
  
  // 保存当前状态
  ctx.save()
  
  // 应用变换（平移和缩放）
  ctx.translate(offsetX.value, offsetY.value)
  ctx.scale(scale.value, scale.value)
  
  // 绘制边
  ctx.lineWidth = 1.5 / scale.value
  for (const edge of edges.value) {
    ctx.beginPath()
    ctx.moveTo(edge.source.x, edge.source.y)
    ctx.lineTo(edge.target.x, edge.target.y)
    ctx.strokeStyle = relationColors[edge.type] || relationColors.default
    ctx.stroke()
    
    // 绘制关系类型标签（如果缩放比例足够大）
    if (scale.value > 0.5) {
      const midX = (edge.source.x + edge.target.x) / 2
      const midY = (edge.source.y + edge.target.y) / 2
      
      // 关系类型映射
      const relationLabels = {
        'cooccurs': '共现',
        'isa': '是',
        'partof': '部分',
        'relatedto': '相关',
        'uses': '使用',
        'works_for': '任职',
        'manages': '管理',
        'depends_on': '依赖',
        'creates': '创建',
        'implements': '实现',
        'extends': '扩展',
        'located_in': '位于',
      }
      
      const label = relationLabels[edge.type] || edge.type
      
      // 绘制标签背景
      ctx.font = '10px sans-serif'
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      const textWidth = ctx.measureText(label).width
      
      ctx.fillStyle = 'rgba(255, 255, 255, 0.95)'
      ctx.fillRect(midX - textWidth / 2 - 3, midY - 7, textWidth + 6, 14)
      
      // 绘制标签文字
      ctx.fillStyle = '#334155'
      ctx.fillText(label, midX, midY)
    }
  }
  
  // 绘制节点
  for (const node of nodes.value) {
    const isSelected = selectedNode.value?.id === node.id
    const isHovered = hoveredNode.value?.id === node.id
    const radius = isSelected || isHovered ? node.radius * 1.5 : node.radius
    
    // 节点阴影
    if (isSelected || isHovered) {
      ctx.beginPath()
      ctx.arc(node.x, node.y, radius + 4, 0, Math.PI * 2)
      ctx.fillStyle = 'rgba(59, 130, 246, 0.2)'
      ctx.fill()
    }
    
    // 节点圆圈
    ctx.beginPath()
    ctx.arc(node.x, node.y, radius, 0, Math.PI * 2)
    ctx.fillStyle = colors[node.type] || colors.default
    ctx.fill()
    
    // 节点边框
    ctx.strokeStyle = isSelected ? '#1e40af' : '#ffffff'
    ctx.lineWidth = (isSelected ? 3 : 2) / scale.value
    ctx.stroke()
    
    // 绘制标签 - 默认显示所有节点名称
    const shouldShowLabel = isHovered || isSelected || scale.value > 0.3
    
    if (shouldShowLabel) {
      ctx.fillStyle = '#1e293b'
      ctx.font = `${isSelected ? 'bold 12px' : '11px'} sans-serif`
      ctx.textAlign = 'center'
      ctx.textBaseline = 'top'
      
      // 文字背景
      const text = node.name
      const textWidth = ctx.measureText(text).width
      ctx.fillStyle = 'rgba(255, 255, 255, 0.9)'
      ctx.fillRect(node.x - textWidth / 2 - 4, node.y + radius + 4, textWidth + 8, 16)
      
      // 文字
      ctx.fillStyle = '#1e293b'
      ctx.fillText(text, node.x, node.y + radius + 6)
    }
  }
  
  // 恢复状态
  ctx.restore()
}

// 动画循环
const animate = () => {
  updatePhysics()
  render()
  animationFrame = requestAnimationFrame(animate)
}

// 鼠标事件
const getTransformedMousePos = (e) => {
  const rect = canvas.value.getBoundingClientRect()
  const x = e.clientX - rect.left
  const y = e.clientY - rect.top
  // 应用逆变换
  const transformedX = (x - offsetX.value) / scale.value
  const transformedY = (y - offsetY.value) / scale.value
  return { x: transformedX, y: transformedY }
}

const handleMouseMove = (e) => {
  const rect = canvas.value.getBoundingClientRect()
  const screenX = e.clientX - rect.left
  const screenY = e.clientY - rect.top
  
  // 处理平移
  if (isPanning.value) {
    const dx = screenX - lastMouseX.value
    const dy = screenY - lastMouseY.value
    offsetX.value += dx
    offsetY.value += dy
    lastMouseX.value = screenX
    lastMouseY.value = screenY
    return
  }
  
  const { x, y } = getTransformedMousePos(e)
  
  // 如果正在拖拽节点
  if (isDragging.value && draggedNode.value) {
    draggedNode.value.x = x
    draggedNode.value.y = y
    return
  }
  
  hoveredNode.value = null
  for (const node of nodes.value) {
    const dx = node.x - x
    const dy = node.y - y
    if (dx * dx + dy * dy < node.radius * node.radius * 4) {
      hoveredNode.value = node
      canvas.value.style.cursor = 'grab'
      return
    }
  }
  canvas.value.style.cursor = 'default'
}

const handleMouseDown = (e) => {
  const rect = canvas.value.getBoundingClientRect()
  const screenX = e.clientX - rect.left
  const screenY = e.clientY - rect.top
  
  // 按住空格键或中键开启平移
  if (e.button === 1 || e.shiftKey) {
    isPanning.value = true
    lastMouseX.value = screenX
    lastMouseY.value = screenY
    canvas.value.style.cursor = 'grabbing'
    e.preventDefault()
    return
  }
  
  const { x, y } = getTransformedMousePos(e)
  
  for (const node of nodes.value) {
    const dx = node.x - x
    const dy = node.y - y
    if (dx * dx + dy * dy < node.radius * node.radius * 4) {
      draggedNode.value = node
      isDragging.value = true
      canvas.value.style.cursor = 'grabbing'
      return
    }
  }
}

const handleMouseUp = () => {
  if (isPanning.value) {
    isPanning.value = false
    canvas.value.style.cursor = 'default'
  }
  
  if (isDragging.value) {
    isDragging.value = false
    draggedNode.value = null
    canvas.value.style.cursor = 'default'
  }
}

const handleWheel = (e) => {
  e.preventDefault()
  
  const rect = canvas.value.getBoundingClientRect()
  const mouseX = e.clientX - rect.left
  const mouseY = e.clientY - rect.top
  
  // 计算缩放
  const zoomFactor = e.deltaY > 0 ? 0.9 : 1.1
  const newScale = Math.max(0.1, Math.min(5, scale.value * zoomFactor))
  
  // 以鼠标位置为中心缩放
  const worldX = (mouseX - offsetX.value) / scale.value
  const worldY = (mouseY - offsetY.value) / scale.value
  
  offsetX.value = mouseX - worldX * newScale
  offsetY.value = mouseY - worldY * newScale
  scale.value = newScale
}

const handleClick = (e) => {
  // 如果刚结束拖拽或平移，不触发点击
  if (isDragging.value || isPanning.value) {
    return
  }
  
  const { x, y } = getTransformedMousePos(e)
  
  for (const node of nodes.value) {
    const dx = node.x - x
    const dy = node.y - y
    if (dx * dx + dy * dy < node.radius * node.radius * 4) {
      selectedNode.value = node
      return
    }
  }
  selectedNode.value = null
}

const handleResize = () => {
  initCanvas()
}

// 重置视图
const resetView = () => {
  selectedNode.value = null
  scale.value = 1
  offsetX.value = 0
  offsetY.value = 0
  loadGraphData()
}

// 缩放控制
const zoomIn = () => {
  const centerX = canvas.value.width / 2
  const centerY = canvas.value.height / 2
  const worldX = (centerX - offsetX.value) / scale.value
  const worldY = (centerY - offsetY.value) / scale.value
  
  const newScale = Math.min(5, scale.value * 1.2)
  offsetX.value = centerX - worldX * newScale
  offsetY.value = centerY - worldY * newScale
  scale.value = newScale
}

const zoomOut = () => {
  const centerX = canvas.value.width / 2
  const centerY = canvas.value.height / 2
  const worldX = (centerX - offsetX.value) / scale.value
  const worldY = (centerY - offsetY.value) / scale.value
  
  const newScale = Math.max(0.1, scale.value / 1.2)
  offsetX.value = centerX - worldX * newScale
  offsetY.value = centerY - worldY * newScale
  scale.value = newScale
}

const fitToScreen = () => {
  if (nodes.value.length === 0) return
  
  // 计算所有节点的边界
  let minX = Infinity, maxX = -Infinity
  let minY = Infinity, maxY = -Infinity
  
  for (const node of nodes.value) {
    minX = Math.min(minX, node.x)
    maxX = Math.max(maxX, node.x)
    minY = Math.min(minY, node.y)
    maxY = Math.max(maxY, node.y)
  }
  
  const padding = 100
  const graphWidth = maxX - minX + padding * 2
  const graphHeight = maxY - minY + padding * 2
  
  // 计算适合的缩放比例
  const scaleX = canvas.value.width / graphWidth
  const scaleY = canvas.value.height / graphHeight
  const newScale = Math.min(scaleX, scaleY, 2)
  
  // 计算偏移使图居中
  const centerX = (minX + maxX) / 2
  const centerY = (minY + maxY) / 2
  
  offsetX.value = canvas.value.width / 2 - centerX * newScale
  offsetY.value = canvas.value.height / 2 - centerY * newScale
  scale.value = newScale
}

onMounted(() => {
  initCanvas()
  loadGraphData()
  animate()
  
  window.addEventListener('resize', handleResize)
})

onBeforeUnmount(() => {
  if (animationFrame) {
    cancelAnimationFrame(animationFrame)
  }
  window.removeEventListener('resize', handleResize)
})

watch(() => props.kbId, () => {
  loadGraphData()
})

const entityTypeMap = {
  'person': { label: '人物', icon: '👤' },
  'organization': { label: '组织', icon: '🏢' },
  'location': { label: '地点', icon: '📍' },
  'date': { label: '日期', icon: '📅' },
  'product': { label: '产品', icon: '📦' },
  'technology': { label: '技术', icon: '⚡' },
  'concept': { label: '概念', icon: '💡' },
  'api': { label: 'API', icon: '🔌' },
}

const getEntityTypeInfo = (type) => {
  return entityTypeMap[type] || { label: type, icon: '📌' }
}
</script>

<template>
  <div class="bg-white rounded-xl border border-slate-200 shadow-sm overflow-hidden">
    <!-- 工具栏 -->
    <div class="px-4 py-3 border-b border-slate-200 bg-slate-50">
      <div class="flex items-center justify-between mb-3">
        <div class="flex items-center gap-3">
          <h3 class="font-semibold text-slate-800">知识图谱可视化</h3>
          <span class="text-sm text-slate-500">
            {{ nodes.length }} / {{ allNodes.length }} 个节点 · {{ edges.length }} / {{ allEdges.length }} 条边
          </span>
        </div>
        <div class="flex items-center gap-2">
          <button
            v-if="searchQuery || selectedEntityTypes.length > 0 || selectedRelationTypes.length > 0"
            @click="clearFilters"
            class="px-3 py-1.5 text-sm bg-white border border-slate-300 rounded-lg hover:bg-slate-50 transition-colors"
          >
            ✕ 清除筛选
          </button>
          <button
            @click="resetView"
            class="px-3 py-1.5 text-sm bg-white border border-slate-300 rounded-lg hover:bg-slate-50 transition-colors"
          >
            🔄 重置
          </button>
        </div>
      </div>
      
      <!-- 搜索框 -->
      <div class="mb-3">
        <input
          v-model="searchQuery"
          type="text"
          placeholder="搜索实体名称..."
          class="w-full px-4 py-2 border border-slate-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
        />
      </div>
      
      <!-- 实体类型筛选 -->
      <div class="mb-3">
        <div class="text-xs font-semibold text-slate-600 mb-2">实体类型</div>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="entityType in availableEntityTypes"
            :key="entityType.type"
            @click="toggleEntityType(entityType.type)"
            :class="[
              'px-3 py-1.5 text-sm rounded-lg border transition-colors',
              selectedEntityTypes.includes(entityType.type)
                ? 'bg-blue-100 border-blue-300 text-blue-700'
                : 'bg-white border-slate-300 text-slate-700 hover:bg-slate-50'
            ]"
          >
            {{ entityType.icon }} {{ entityType.label }} ({{ entityType.count }})
          </button>
        </div>
      </div>
      
      <!-- 关系类型筛选 -->
      <div v-if="availableRelationTypes.length > 0">
        <div class="text-xs font-semibold text-slate-600 mb-2">关系类型</div>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="relationType in availableRelationTypes"
            :key="relationType.type"
            @click="toggleRelationType(relationType.type)"
            :class="[
              'px-3 py-1.5 text-sm rounded-lg border transition-colors',
              selectedRelationTypes.includes(relationType.type)
                ? 'bg-purple-100 border-purple-300 text-purple-700'
                : 'bg-white border-slate-300 text-slate-700 hover:bg-slate-50'
            ]"
          >
            {{ relationType.label }} ({{ relationType.count }})
          </button>
        </div>
      </div>
    </div>
    
    <!-- 画布容器 -->
    <div ref="container" class="relative" style="height: 600px;">
      <canvas
        ref="canvas"
        @mousemove="handleMouseMove"
        @mousedown="handleMouseDown"
        @mouseup="handleMouseUp"
        @wheel="handleWheel"
        @click="handleClick"
        class="w-full h-full"
      ></canvas>
      
      <!-- 加载状态 -->
      <div v-if="loading" class="absolute inset-0 flex items-center justify-center bg-white bg-opacity-90">
        <div class="text-center">
          <div class="inline-block animate-spin rounded-full h-8 w-8 border-b-2 border-blue-500"></div>
          <p class="mt-2 text-slate-500">加载图谱数据...</p>
        </div>
      </div>
      
      <!-- 选中节点信息 -->
      <div
        v-if="selectedNode"
        class="absolute top-4 right-4 bg-white rounded-lg shadow-lg border border-slate-200 p-4 max-w-xs"
      >
        <div class="flex items-start justify-between mb-2">
          <div class="flex items-center gap-2">
            <span class="text-2xl">{{ getEntityTypeInfo(selectedNode.type).icon }}</span>
            <div>
              <h4 class="font-semibold text-slate-800">{{ selectedNode.name }}</h4>
              <p class="text-xs text-slate-500">{{ getEntityTypeInfo(selectedNode.type).label }}</p>
            </div>
          </div>
          <button
            @click="selectedNode = null"
            class="text-slate-400 hover:text-slate-600"
          >
            ✕
          </button>
        </div>
        <div class="text-xs text-slate-600 space-y-1">
          <p><strong>ID:</strong> {{ selectedNode.id }}</p>
          <p v-if="selectedNode.entity.file_id"><strong>文件ID:</strong> {{ selectedNode.entity.file_id }}</p>
        </div>
      </div>
      
      <!-- 图例 -->
      <div class="absolute bottom-4 left-4 bg-white rounded-lg shadow-lg border border-slate-200 p-3 max-h-96 overflow-y-auto">
        <h4 class="text-xs font-semibold text-slate-700 mb-2">图例</h4>
        
        <!-- 实体类型 -->
        <div class="mb-3">
          <div class="text-xs font-medium text-slate-600 mb-1.5">实体类型</div>
          <div class="space-y-1.5 text-xs">
            <div class="flex items-center gap-2">
              <div class="w-3 h-3 rounded-full bg-cyan-500"></div>
              <span class="text-slate-600">技术</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-3 h-3 rounded-full bg-yellow-500"></div>
              <span class="text-slate-600">概念</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-3 h-3 rounded-full bg-blue-500"></div>
              <span class="text-slate-600">人物</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-3 h-3 rounded-full bg-purple-500"></div>
              <span class="text-slate-600">组织</span>
            </div>
          </div>
        </div>
        
        <!-- 关系类型 -->
        <div>
          <div class="text-xs font-medium text-slate-600 mb-1.5">关系类型</div>
          <div class="space-y-1.5 text-xs">
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-blue-300"></div>
              <span class="text-slate-600">共现</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-purple-300"></div>
              <span class="text-slate-600">是/分类</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-green-300"></div>
              <span class="text-slate-600">部分</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-cyan-300"></div>
              <span class="text-slate-600">相关</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-yellow-300"></div>
              <span class="text-slate-600">使用</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-pink-300"></div>
              <span class="text-slate-600">任职</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-orange-300"></div>
              <span class="text-slate-600">管理</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-4 h-0.5 bg-slate-300"></div>
              <span class="text-slate-600">其他</span>
            </div>
          </div>
        </div>
      </div>
      
      <!-- 缩放控制 -->
      <div class="absolute bottom-4 right-4 bg-white rounded-lg shadow-lg border border-slate-200 p-2 flex flex-col gap-2">
        <button
          @click="zoomIn"
          class="w-10 h-10 flex items-center justify-center text-lg hover:bg-slate-100 rounded transition-colors"
          title="放大 (滚轮向上)"
        >
          +
        </button>
        <div class="text-xs text-center text-slate-500 px-2">
          {{ Math.round(scale * 100) }}%
        </div>
        <button
          @click="zoomOut"
          class="w-10 h-10 flex items-center justify-center text-lg hover:bg-slate-100 rounded transition-colors"
          title="缩小 (滚轮向下)"
        >
          −
        </button>
        <div class="border-t border-slate-200 my-1"></div>
        <button
          @click="fitToScreen"
          class="w-10 h-10 flex items-center justify-center text-sm hover:bg-slate-100 rounded transition-colors"
          title="适应屏幕"
        >
          ⊡
        </button>
      </div>
      
      <!-- 操作提示 -->
      <div class="absolute top-4 left-4 bg-white/90 rounded-lg shadow border border-slate-200 px-3 py-2 text-xs text-slate-600">
        <div>💡 <strong>拖拽节点</strong>: 点击并拖动</div>
        <div>💡 <strong>平移视图</strong>: Shift + 拖动 或 中键拖动</div>
        <div>💡 <strong>缩放</strong>: 滚轮滚动</div>
      </div>
      
      <!-- 提示 -->
      <div v-if="!loading && nodes.length === 0" class="absolute inset-0 flex items-center justify-center">
        <div class="text-center text-slate-400">
          <span class="text-4xl block mb-2">🕸️</span>
          <p>暂无图谱数据</p>
          <p class="text-sm mt-1">上传文档后将自动生成知识图谱</p>
        </div>
      </div>
    </div>
  </div>
</template>
