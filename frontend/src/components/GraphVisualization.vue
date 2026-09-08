<script setup>
import { ref, onMounted, onBeforeUnmount, computed, watch } from 'vue'
import { api } from '../api.js'

const props = defineProps({
  kbId: {
    type: Number,
    default: null
  },
  fileId: {
    type: Number,
    default: null
  },
  query: {
    type: String,
    default: null
  },
  entityType: {
    type: String,
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
const selectedEdge = ref(null)
const edgeEvidence = ref([])
const evidenceLoading = ref(false)
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

// 搜索匹配的节点ID集合（用于高亮）
const matchedNodeIds = ref(new Set())

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

// 调整颜色透明度
const adjustColorOpacity = (hexColor, opacity) => {
  // 将 hex 转换为 rgba
  const r = parseInt(hexColor.slice(1, 3), 16)
  const g = parseInt(hexColor.slice(3, 5), 16)
  const b = parseInt(hexColor.slice(5, 7), 16)
  return `rgba(${r}, ${g}, ${b}, ${opacity})`
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
let graphRequest = 0
let graphAbort = null
let physicsSteps = 0
const loadError = ref('')
const truncated = ref(false)
const expanding = new Set()
const NODE_LIMIT = 200
const EDGE_LIMIT = 400

const mergeSubgraph = (data, replace = false) => {
  const nodeMap = new Map((replace ? [] : allNodes.value).map(n => [n.id, n]))
  const matched = new Set(replace ? data.matched_ids : [...matchedNodeIds.value, ...data.matched_ids])
  for (const entity of data.nodes) {
    if (nodeMap.has(entity.id)) continue
    if (nodeMap.size >= NODE_LIMIT) { truncated.value = true; break }
    nodeMap.set(entity.id, {
      id: entity.id, name: entity.name, type: entity.entity_type, entity,
      x: Math.random() * canvas.value.width, y: Math.random() * canvas.value.height,
      vx: 0, vy: 0, radius: matched.has(entity.id) ? 8 : 6, isMatched: matched.has(entity.id)
    })
  }
  const edgeMap = new Map((replace ? [] : allEdges.value).map(e => [e.id, e]))
  for (const edge of data.edges) {
    if (!nodeMap.has(edge.source_id) || !nodeMap.has(edge.target_id)) continue
    if (edgeMap.size >= EDGE_LIMIT && !edgeMap.has(edge.id)) { truncated.value = true; break }
    edgeMap.set(edge.id, { id: edge.id, source: nodeMap.get(edge.source_id), target: nodeMap.get(edge.target_id), type: edge.relation_type })
  }
  matchedNodeIds.value = matched
  allNodes.value = [...nodeMap.values()]
  allEdges.value = [...edgeMap.values()]
  // Give parallel and reverse relations separate curves and labels.
  const groups = new Map()
  for (const edge of allEdges.value) {
    const key = [edge.source.id, edge.target.id].sort((a, b) => a - b).join(':')
    if (!groups.has(key)) groups.set(key, [])
    groups.get(key).push(edge)
  }
  for (const group of groups.values()) {
    group.sort((a, b) => a.id - b.id)
    group.forEach((edge, i) => { edge.curve = (i - (group.length - 1) / 2) * 40 * (edge.source.id <= edge.target.id ? 1 : -1) })
  }
  truncated.value ||= data.truncated
  applyFilters()
}

const loadGraphData = async () => {
  const request = ++graphRequest
  graphAbort?.abort()
  graphAbort = new AbortController()
  expanding.clear()
  loading.value = true
  loadError.value = ''
  truncated.value = false
  selectedNode.value = null
  selectedEdge.value = null
  allNodes.value = []
  allEdges.value = []
  applyFilters()
  try {
    const data = await api.getSubgraph({
      q: props.query, entity_type: props.entityType, kb_id: props.kbId,
      file_id: props.fileId, limit: Math.max(1, Math.min(200, props.maxNodes))
    }, graphAbort.signal)
    if (request !== graphRequest) return
    mergeSubgraph(data, true)
  } catch (error) {
    if (request === graphRequest && error.name !== 'AbortError') loadError.value = error.message
  } finally {
    if (request === graphRequest) loading.value = false
  }
}

const expandNode = async (node) => {
  if (!node || expanding.has(node.id)) return
  if (allNodes.value.length >= NODE_LIMIT || allEdges.value.length >= EDGE_LIMIT) { truncated.value = true; return }
  const request = graphRequest
  expanding.add(node.id)
  try {
    const data = await api.getSubgraph({ node_id: node.id, kb_id: props.kbId, file_id: props.fileId, limit: 1 }, graphAbort?.signal)
    if (request !== graphRequest) return
    mergeSubgraph(data)
  } catch (error) {
    if (request === graphRequest && error.name !== 'AbortError') loadError.value = error.message
  } finally {
    if (request === graphRequest) expanding.delete(node.id)
  }
}

// 应用筛选
const applyFilters = () => {
  physicsSteps = 240
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
    ...entityTypeMap[type] || { label: type, icon: '·' },
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
  if (physicsSteps-- <= 0 && !isDragging.value) return
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
    const dx = edge.target.x - edge.source.x
    const dy = edge.target.y - edge.source.y
    const length = Math.hypot(dx, dy) || 1
    const controlX = (edge.source.x + edge.target.x) / 2 - dy / length * (edge.curve || 0)
    const controlY = (edge.source.y + edge.target.y) / 2 + dx / length * (edge.curve || 0)
    ctx.quadraticCurveTo(controlX, controlY, edge.target.x, edge.target.y)
    ctx.strokeStyle = relationColors[edge.type] || relationColors.default
    ctx.stroke()
    const angle = Math.atan2(edge.target.y - controlY, edge.target.x - controlX)
    const tipX = edge.target.x - Math.cos(angle) * (edge.target.radius + 3)
    const tipY = edge.target.y - Math.sin(angle) * (edge.target.radius + 3)
    ctx.beginPath()
    ctx.moveTo(tipX, tipY)
    ctx.lineTo(tipX - 9 * Math.cos(angle - 0.4), tipY - 9 * Math.sin(angle - 0.4))
    ctx.lineTo(tipX - 9 * Math.cos(angle + 0.4), tipY - 9 * Math.sin(angle + 0.4))
    ctx.closePath()
    ctx.fillStyle = ctx.strokeStyle
    ctx.fill()
    
    // 绘制关系类型标签（如果缩放比例足够大）
    if (scale.value > 0.5) {
      const midX = (edge.source.x + 2 * controlX + edge.target.x) / 4
      const midY = (edge.source.y + 2 * controlY + edge.target.y) / 4
      edge.labelX = midX
      edge.labelY = midY
      
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
    const isMatched = node.isMatched  // 搜索匹配的节点
    const radius = isSelected || isHovered ? node.radius * 1.5 : node.radius

    // 搜索匹配节点的外发光效果
    if (isMatched && !isSelected && !isHovered) {
      ctx.beginPath()
      ctx.arc(node.x, node.y, radius + 6, 0, Math.PI * 2)
      ctx.fillStyle = 'rgba(251, 191, 36, 0.3)'  // 金色光晕
      ctx.fill()
      ctx.beginPath()
      ctx.arc(node.x, node.y, radius + 3, 0, Math.PI * 2)
      ctx.fillStyle = 'rgba(251, 191, 36, 0.4)'
      ctx.fill()
    }

    // 节点阴影（选中或悬停）
    if (isSelected || isHovered) {
      ctx.beginPath()
      ctx.arc(node.x, node.y, radius + 4, 0, Math.PI * 2)
      ctx.fillStyle = 'rgba(59, 130, 246, 0.3)'
      ctx.fill()
    }

    // 节点圆圈
    ctx.beginPath()
    ctx.arc(node.x, node.y, radius, 0, Math.PI * 2)
    // 关联节点使用半透明颜色
    const baseColor = colors[node.type] || colors.default
    ctx.fillStyle = isMatched ? baseColor : adjustColorOpacity(baseColor, 0.6)
    ctx.fill()

    // 节点边框
    if (isMatched) {
      // 匹配节点用金色边框
      ctx.strokeStyle = isSelected ? '#1e40af' : '#f59e0b'
      ctx.lineWidth = (isSelected ? 3 : 2.5) / scale.value
    } else {
      // 关联节点用白色虚线边框
      ctx.strokeStyle = isSelected ? '#1e40af' : '#ffffff'
      ctx.lineWidth = (isSelected ? 3 : 1.5) / scale.value
    }
    ctx.stroke()

    // 绘制标签
    const shouldShowLabel = isHovered || isSelected || isMatched || scale.value > 0.5

    if (shouldShowLabel) {
      ctx.fillStyle = '#1e293b'
      ctx.font = `${isSelected || isMatched ? 'bold 12px' : '10px'} sans-serif`
      ctx.textAlign = 'center'
      ctx.textBaseline = 'top'

      // 文字背景
      const text = node.name
      const textWidth = ctx.measureText(text).width
      ctx.fillStyle = isMatched ? 'rgba(254, 243, 199, 0.95)' : 'rgba(255, 255, 255, 0.85)'
      ctx.fillRect(node.x - textWidth / 2 - 4, node.y + radius + 4, textWidth + 8, 16)

      // 文字
      ctx.fillStyle = isMatched ? '#92400e' : '#64748b'
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
  physicsSteps = 240
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
      selectedEdge.value = null
      // 点击节点时展开其关联节点
      expandNode(node)
      return
    }
  }
  selectedNode.value = null
  selectedEdge.value = null
  if (scale.value <= 0.5) return
  const edge = edges.value.find(e => Math.abs(e.labelX - x) < 35 && Math.abs(e.labelY - y) < 12)
  if (!edge) return
  selectedEdge.value = edge
  edgeEvidence.value = []
  evidenceLoading.value = true
  const request = graphRequest
  api.getEdgeEvidence(edge.id, graphAbort?.signal).then(evidence => {
    if (request === graphRequest && selectedEdge.value?.id === edge.id) edgeEvidence.value = evidence
  }).catch(error => {
    if (request === graphRequest && error.name !== 'AbortError') loadError.value = error.message
  }).finally(() => {
    if (request === graphRequest && selectedEdge.value?.id === edge.id) evidenceLoading.value = false
  })
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
  ++graphRequest
  graphAbort?.abort()
  if (animationFrame) {
    cancelAnimationFrame(animationFrame)
  }
  window.removeEventListener('resize', handleResize)
})

watch(() => props.kbId, () => {
  loadGraphData()
})

watch(() => props.fileId, () => {
  loadGraphData()
})

watch(() => props.query, () => {
  loadGraphData()
})

watch(() => props.entityType, () => {
  loadGraphData()
})

const entityTypeMap = {
  'person': { label: '人物', icon: '人' },
  'organization': { label: '组织', icon: '组织' },
  'location': { label: '地点', icon: '地' },
  'date': { label: '日期', icon: '日' },
  'product': { label: '产品', icon: '品' },
  'technology': { label: '技术', icon: '技' },
  'concept': { label: '概念', icon: '概' },
  'api': { label: 'API', icon: 'API' },
}

const getEntityTypeInfo = (type) => {
  return entityTypeMap[type] || { label: type, icon: '·' }
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
            重置
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
    
    <p v-if="loadError" class="px-4 py-2 text-sm text-red-600">{{ loadError }}</p>
    <p v-if="truncated" class="px-4 py-2 text-sm text-slate-500">当前展示部分图谱，最多 200 个节点、400 条关系。可缩小搜索范围后继续浏览。</p>
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
      
      <div v-if="selectedEdge" class="absolute top-4 right-4 bg-white rounded-lg shadow-lg border border-slate-200 p-4 max-w-sm max-h-80 overflow-auto">
        <button class="float-right text-slate-400" @click="selectedEdge = null">✕</button>
        <p class="text-sm font-semibold pr-5">{{ selectedEdge.source.name }} → {{ selectedEdge.type }} → {{ selectedEdge.target.name }}</p>
        <p v-if="evidenceLoading" class="text-xs text-slate-500 mt-2">加载来源…</p>
        <p v-else-if="!edgeEvidence.length" class="text-xs text-slate-500 mt-2">此关系尚无原文证据；旧图谱需重新构建。</p>
        <div v-for="(item, i) in edgeEvidence" :key="i" class="mt-3 text-xs">
          <p class="font-medium">{{ item.filename }}</p>
          <blockquote class="mt-1 border-l-2 pl-2 whitespace-pre-wrap">{{ item.context }}</blockquote>
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

        <!-- 节点状态 -->
        <div class="mb-3">
          <div class="text-xs font-medium text-slate-600 mb-1.5">节点状态</div>
          <div class="space-y-1.5 text-xs">
            <div class="flex items-center gap-2">
              <div class="w-4 h-4 rounded-full bg-blue-500 border-2 border-amber-400 shadow-[0_0_6px_rgba(251,191,36,0.5)]"></div>
              <span class="text-slate-600">搜索匹配</span>
            </div>
            <div class="flex items-center gap-2">
              <div class="w-3 h-3 rounded-full bg-blue-300 border border-white opacity-70"></div>
              <span class="text-slate-600">关联实体</span>
            </div>
          </div>
        </div>

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
        <div><strong>点击节点</strong>: 展开关联实体</div>
        <div><strong>点击关系文字</strong>: 查看原文证据</div>
        <div><strong>拖拽节点</strong>: 点击并拖动</div>
        <div><strong>平移视图</strong>: Shift + 拖动 或 中键拖动</div>
        <div><strong>缩放</strong>: 滚轮滚动</div>
      </div>
      
      <!-- 提示 -->
      <div v-if="!loading && nodes.length === 0" class="absolute inset-0 flex items-center justify-center">
        <div class="text-center text-slate-400">
          <span class="text-xl block mb-2 font-semibold">GRAPH</span>
          <p>暂无图谱数据</p>
          <p class="text-sm mt-1">启用图谱构建并成功处理文档后，可在此查看</p>
        </div>
      </div>
    </div>
  </div>
</template>
