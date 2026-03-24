<script setup>
import { ref, computed } from 'vue'
import { api } from '../api'
import FileSlices from './FileSlices.vue'
import FileGraph from './FileGraph.vue'
import KnowledgeBaseSelector from './KnowledgeBaseSelector.vue'

const props = defineProps({
  file: {
    type: Object,
    required: true
  },
  kbType: {
    type: String,
    default: null
  }
})

const emit = defineEmits(['updated', 'deleted'])

const showSlices = ref(false)
const showSettings = ref(false)
const showGraph = ref(false)
const showTagsEditor = ref(false)
const showMoveKbSelector = ref(false)
const selectedSliceType = ref(props.file.slice_type || 'paragraph')
const updating = ref(false)
const downloading = ref(false)
const moving = ref(false)
const editingTags = ref([])
const newTag = ref('')

const sliceTypes = [
  { value: 'smart', label: '智能切片', desc: '根据文档结构智能切分（推荐）' },
  { value: 'fixed', label: '固定长度', desc: '每 8000 字符一个切片，重叠 100 字' },
]

const isStorageKb = computed(() => props.kbType === 'storage')

const statusInfo = computed(() => {
  switch (props.file.status) {
    case 0:
      return { text: '待处理', color: 'bg-amber-100 text-amber-700', icon: '⏳' }
    case 2:
      return { text: '处理中', color: 'bg-blue-100 text-blue-700', icon: '⚙️' }
    case 1:
      return { text: '已完成', color: 'bg-green-100 text-green-700', icon: '✓' }
    case 3:
      return { text: '不解析', color: 'bg-amber-100 text-amber-700', icon: '🗄️' }
    case -1:
      return { text: '处理失败', color: 'bg-red-100 text-red-700', icon: '✗' }
    default:
      return { text: '未知', color: 'bg-slate-100 text-slate-700', icon: '?' }
  }
})

const publicInfo = computed(() => {
  return {
    isPublic: props.file.is_public,
    text: props.file.is_public ? '公开' : '私有',
    color: props.file.is_public ? 'bg-green-50 text-green-600 border-green-200' : 'bg-slate-50 text-slate-600 border-slate-200',
    icon: props.file.is_public ? '🌐' : '🔒'
  }
})

const sliceTypeLabel = computed(() => {
  if (isStorageKb.value) {
    return '存储模式'
  }
  const type = sliceTypes.find(t => t.value === props.file.slice_type)
  return type ? type.label : '智能切片'
})

const fileTags = computed(() => {
  if (!props.file.tags) return []
  try {
    return JSON.parse(props.file.tags)
  } catch {
    return []
  }
})

const formatDate = (timestamp) => {
  if (!timestamp) return '-'
  return new Date(timestamp * 1000).toLocaleString('zh-CN')
}

const handleUpdateSliceType = async () => {
  const currentSliceType = props.file.slice_type || 'smart'
  const isSameSliceType = selectedSliceType.value === currentSliceType
  if (isSameSliceType) {
    const shouldReparse = confirm('切片方式未变化，是否重新解析该文件？')
    if (!shouldReparse) return
  }

  updating.value = true
  try {
    await api.updateFile(props.file.id, { slice_type: selectedSliceType.value })
    showSettings.value = false
    emit('updated')
  } catch (e) {
    alert('更新失败：' + e.message)
  } finally {
    updating.value = false
  }
}

const handleDelete = async () => {
  if (!confirm('确定要删除这个文件吗？')) return

  try {
    await api.deleteFile(props.file.id)
    emit('deleted')
  } catch (e) {
    alert('删除失败：' + e.message)
  }
}

const handleDownload = async () => {
  if (downloading.value) return
  downloading.value = true
  try {
    const blob = await api.downloadFile(props.file.id)
    const url = window.URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = props.file.filename || 'download'
    document.body.appendChild(link)
    link.click()
    link.remove()
    window.URL.revokeObjectURL(url)
  } catch (e) {
    alert('下载失败：' + (e?.message || '未知错误'))
  } finally {
    downloading.value = false
  }
}

const openTagsEditor = () => {
  editingTags.value = [...fileTags.value]
  newTag.value = ''
  showTagsEditor.value = true
}

const addTag = () => {
  const tag = newTag.value.trim()
  if (tag && !editingTags.value.includes(tag)) {
    editingTags.value.push(tag)
    newTag.value = ''
  }
}

const removeTag = (index) => {
  editingTags.value.splice(index, 1)
}

const handleSaveTags = async () => {
  updating.value = true
  try {
    await api.updateFile(props.file.id, { tags: editingTags.value })
    showTagsEditor.value = false
    emit('updated')
  } catch (e) {
    alert('更新标签失败：' + e.message)
  } finally {
    updating.value = false
  }
}

const handleTogglePublic = async () => {
  const newPublic = !publicInfo.value.isPublic
  if (!confirm(`确定要将文件设置为${newPublic ? '公开' : '私有'}吗？`)) return

  updating.value = true
  try {
    await api.updateFile(props.file.id, { is_public: newPublic })
    emit('updated')
  } catch (e) {
    alert('更新失败：' + e.message)
  } finally {
    updating.value = false
  }
}

const openMoveSelector = () => {
  if (moving.value || props.file.status === 2) return
  showMoveKbSelector.value = true
}

const handleMoveToKb = async (kb) => {
  const targetKbId = kb?.id ?? null
  const currentKbId = props.file.kb_id ?? null
  if (targetKbId === currentKbId) {
    alert('文件已在当前知识库中')
    return
  }

  const targetName = kb?.name || '未分配知识库'
  if (!confirm(`确定要将文件移动到「${targetName}」吗？移动后会重新进入解析流程。`)) return

  moving.value = true
  try {
    await api.moveFile(props.file.id, targetKbId)
    emit('updated')
  } catch (e) {
    alert('移动失败：' + e.message)
  } finally {
    moving.value = false
  }
}
</script>

<template>
  <div class="bg-white rounded-xl border border-slate-200 hover:border-slate-300 transition-all duration-200">
    <!-- File Info -->
    <div class="p-4">
      <div class="flex items-start gap-4">
        <!-- File Icon -->
        <div class="w-10 h-10 bg-linear-to-br from-amber-100 to-orange-100 rounded-lg flex items-center justify-center shrink-0">
          <span class="text-lg">📄</span>
        </div>

        <!-- File Details -->
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 mb-1">
            <h3 class="font-medium text-slate-800 truncate">{{ file.filename }}</h3>
            <span :class="['px-2 py-0.5 text-xs rounded-full', statusInfo.color]">
              {{ statusInfo.icon }} {{ statusInfo.text }}
            </span>
            <span :class="['px-2 py-0.5 text-xs rounded-full border', publicInfo.color]">
              {{ publicInfo.icon }} {{ publicInfo.text }}
            </span>
          </div>

          <div class="flex items-center gap-4 text-xs text-slate-400">
            <span>切片方式：{{ sliceTypeLabel }}</span>
            <span>创建时间：{{ formatDate(file.created_at) }}</span>
          </div>

          <!-- Tags -->
          <div v-if="fileTags.length > 0" class="flex items-center gap-2 mt-2 flex-wrap">
            <span
              v-for="tag in fileTags"
              :key="tag"
              class="px-2 py-0.5 text-xs bg-blue-50 text-blue-600 rounded-full border border-blue-100"
            >
              🏷️ {{ tag }}
            </span>
          </div>

          <!-- Error Log -->
          <div v-if="file.status === -1 && file.log" class="mt-2 p-2 bg-red-50 rounded-lg">
            <p class="text-xs text-red-600">{{ file.log }}</p>
          </div>
        </div>

        <!-- Actions -->
        <div class="flex items-center gap-1">
          <button
            @click="handleTogglePublic"
            class="p-2 text-slate-400 hover:text-green-500 hover:bg-green-50 rounded-lg transition-all"
            :title="publicInfo.isPublic ? '设置为私有' : '设置为公开'"
          >
            <svg v-if="publicInfo.isPublic" class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
            </svg>
            <svg v-else class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3.055 11H5a2 2 0 012 2v1a2 2 0 002 2 2 2 0 012 2v2.945M8 3.935V5.5A2.5 2.5 0 0010.5 8h.5a2 2 0 012 2 2 2 0 104 0 2 2 0 012-2h1.064M15 20.488V18a2 2 0 012-2h3.064M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
            </svg>
          </button>
          <button
            @click="openTagsEditor"
            class="p-2 text-slate-400 hover:text-blue-500 hover:bg-blue-50 rounded-lg transition-all"
            title="管理标签"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A1.994 1.994 0 013 12V7a4 4 0 014-4z" />
            </svg>
          </button>
          <button
            v-if="file.status === 1 && !isStorageKb"
            @click="showGraph = true"
            class="p-2 text-slate-400 hover:text-purple-500 hover:bg-purple-50 rounded-lg transition-all"
            title="查看知识图谱"
          >
            <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="3" />
              <circle cx="4" cy="8" r="2" />
              <circle cx="20" cy="8" r="2" />
              <circle cx="4" cy="16" r="2" />
              <circle cx="20" cy="16" r="2" />
              <line x1="6" y1="8" x2="9" y2="10" />
              <line x1="18" y1="8" x2="15" y2="10" />
              <line x1="6" y1="16" x2="9" y2="14" />
              <line x1="18" y1="16" x2="15" y2="14" />
            </svg>
          </button>
          <button
            v-if="file.status === 1 && !isStorageKb"
            @click="showSlices = true"
            class="p-2 text-slate-400 hover:text-blue-500 hover:bg-blue-50 rounded-lg transition-all"
            title="查看切片"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 10h16M4 14h16M4 18h16" />
            </svg>
          </button>
          <button
            v-if="!isStorageKb"
            @click="showSettings = true; selectedSliceType = file.slice_type || 'smart'"
            class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-all"
            title="修改切片方式"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
            </svg>
          </button>
          <button
            @click="openMoveSelector"
            :disabled="moving || file.status === 2"
            class="p-2 text-slate-400 hover:text-indigo-500 hover:bg-indigo-50 rounded-lg transition-all disabled:opacity-50 disabled:cursor-not-allowed"
            :title="file.status === 2 ? '处理中不可移动' : (moving ? '移动中...' : '移动到其他知识库')"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7h5l2 2h11v8a2 2 0 01-2 2H3V7z" />
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 13h6m0 0l-2-2m2 2l-2 2" />
            </svg>
          </button>
          <button
            @click="handleDownload"
            :disabled="downloading"
            class="p-2 text-slate-400 hover:text-emerald-500 hover:bg-emerald-50 rounded-lg transition-all disabled:opacity-50 disabled:cursor-not-allowed"
            :title="downloading ? '下载中...' : '下载'"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 3v12m0 0l4-4m-4 4l-4-4M4 17v2a2 2 0 002 2h12a2 2 0 002-2v-2" />
            </svg>
          </button>
          <button
            @click="handleDelete"
            class="p-2 text-slate-400 hover:text-red-500 hover:bg-red-50 rounded-lg transition-all"
            title="删除"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
            </svg>
          </button>
        </div>
      </div>
    </div>

    <!-- Modals -->
    <Teleport to="body">
      <!-- Tags Editor Modal -->
      <div v-if="showTagsEditor" class="fixed inset-0 z-50 flex items-center justify-center p-4">
        <div @click="showTagsEditor = false" class="absolute inset-0 bg-black/50 backdrop-blur-sm"></div>
        <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-md p-6">
          <div class="flex items-center justify-between mb-5">
            <h3 class="text-lg font-semibold text-slate-800">管理标签</h3>
            <button @click="showTagsEditor = false" class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg">
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <!-- Add Tag Input -->
          <div class="mb-4">
            <div class="flex gap-2">
              <input
                v-model="newTag"
                @keyup.enter="addTag"
                type="text"
                placeholder="输入标签名称..."
                class="flex-1 px-3 py-2 border border-slate-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
              />
              <button
                @click="addTag"
                class="px-4 py-2 bg-blue-500 text-white rounded-lg font-medium hover:bg-blue-600 transition-all"
              >
                添加
              </button>
            </div>
          </div>

          <!-- Tags List -->
          <div class="mb-6">
            <p class="text-sm text-slate-600 mb-2">当前标签：</p>
            <div v-if="editingTags.length === 0" class="text-center py-8 text-slate-400">
              <svg class="w-12 h-12 mx-auto mb-2 opacity-50" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 7h.01M7 3h5c.512 0 1.024.195 1.414.586l7 7a2 2 0 010 2.828l-7 7a2 2 0 01-2.828 0l-7-7A1.994 1.994 0 013 12V7a4 4 0 014-4z" />
              </svg>
              <p>暂无标签</p>
            </div>
            <div v-else class="flex flex-wrap gap-2">
              <div
                v-for="(tag, index) in editingTags"
                :key="index"
                class="flex items-center gap-1 px-3 py-1.5 bg-blue-50 text-blue-600 rounded-lg border border-blue-100 group"
              >
                <span class="text-sm">{{ tag }}</span>
                <button
                  @click="removeTag(index)"
                  class="ml-1 p-0.5 text-blue-400 hover:text-red-500 hover:bg-red-50 rounded transition-all"
                >
                  <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
                  </svg>
                </button>
              </div>
            </div>
          </div>

          <div class="flex gap-3">
            <button
              @click="showTagsEditor = false"
              class="flex-1 py-3 bg-slate-100 text-slate-700 rounded-xl font-medium hover:bg-slate-200"
            >
              取消
            </button>
            <button
              @click="handleSaveTags"
              :disabled="updating"
              class="flex-1 py-3 bg-linear-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 disabled:opacity-50"
            >
              {{ updating ? '保存中...' : '保存' }}
            </button>
          </div>
        </div>
      </div>

      <!-- Slice Settings Modal -->
      <div v-if="showSettings" class="fixed inset-0 z-50 flex items-center justify-center p-4">
        <div @click="showSettings = false" class="absolute inset-0 bg-black/50 backdrop-blur-sm"></div>
        <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-md p-6">
          <div class="flex items-center justify-between mb-5">
            <h3 class="text-lg font-semibold text-slate-800">修改切片方式</h3>
            <button @click="showSettings = false" class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg">
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <div class="space-y-2 mb-6">
            <label
              v-for="type in sliceTypes"
              :key="type.value"
              :class="[
                'flex items-center gap-3 p-3 rounded-xl border-2 cursor-pointer transition-all',
                selectedSliceType === type.value
                  ? 'border-blue-500 bg-blue-50'
                  : 'border-slate-200 hover:border-slate-300'
              ]"
            >
              <input
                type="radio"
                v-model="selectedSliceType"
                :value="type.value"
                class="w-4 h-4 text-blue-500"
              />
              <div>
                <p class="font-medium text-slate-800">{{ type.label }}</p>
                <p class="text-xs text-slate-500">{{ type.desc }}</p>
              </div>
            </label>
          </div>

          <div class="bg-amber-50 border border-amber-200 rounded-xl p-3 mb-6">
            <p class="text-sm text-amber-700">
              <span class="font-medium">注意：</span>修改切片方式将删除现有切片并重新处理文件
            </p>
          </div>

          <div class="flex gap-3">
            <button
              @click="showSettings = false"
              class="flex-1 py-3 bg-slate-100 text-slate-700 rounded-xl font-medium hover:bg-slate-200"
            >
              取消
            </button>
            <button
              @click="handleUpdateSliceType"
              :disabled="updating"
              class="flex-1 py-3 bg-linear-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 disabled:opacity-50"
            >
              {{ updating ? '更新中...' : '确认修改' }}
            </button>
          </div>
        </div>
      </div>
    </Teleport>

    <!-- Slices Modal -->
    <FileSlices
      v-if="showSlices"
      :file="file"
      @close="showSlices = false"
    />

    <!-- Graph Modal -->
    <FileGraph
      v-if="showGraph"
      :file="file"
      @close="showGraph = false"
    />

    <KnowledgeBaseSelector
      :show="showMoveKbSelector"
      @close="showMoveKbSelector = false"
      @select="handleMoveToKb"
    />
  </div>
</template>
