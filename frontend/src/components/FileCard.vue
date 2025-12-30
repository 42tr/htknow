<script setup>
import { ref, computed } from 'vue'
import { api } from '../api'
import FileSlices from './FileSlices.vue'

const props = defineProps({
  file: {
    type: Object,
    required: true
  }
})

const emit = defineEmits(['updated', 'deleted'])

const showSlices = ref(false)
const showSettings = ref(false)
const selectedSliceType = ref(props.file.slice_type || 'paragraph')
const updating = ref(false)

const sliceTypes = [
  { value: 'paragraph', label: '按段落', desc: '以双换行符分隔' },
  { value: 'sentence', label: '按句子', desc: '以句号、问号、感叹号分隔' },
  { value: 'fixed', label: '固定长度', desc: '每 1000 字符一个切片' },
  { value: 'default', label: '不切片', desc: '整个文件作为一个切片' },
]

const statusInfo = computed(() => {
  switch (props.file.status) {
    case 0:
      return { text: '待处理', color: 'bg-amber-100 text-amber-700', icon: '⏳' }
    case 1:
      return { text: '已完成', color: 'bg-green-100 text-green-700', icon: '✓' }
    case -1:
      return { text: '处理失败', color: 'bg-red-100 text-red-700', icon: '✗' }
    default:
      return { text: '未知', color: 'bg-slate-100 text-slate-700', icon: '?' }
  }
})

const sliceTypeLabel = computed(() => {
  const type = sliceTypes.find(t => t.value === props.file.slice_type)
  return type ? type.label : '按段落'
})

const formatDate = (timestamp) => {
  if (!timestamp) return '-'
  return new Date(timestamp * 1000).toLocaleString('zh-CN')
}

const handleUpdateSliceType = async () => {
  if (selectedSliceType.value === props.file.slice_type) {
    showSettings.value = false
    return
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
</script>

<template>
  <div class="bg-white rounded-xl border border-slate-200 hover:border-slate-300 transition-all duration-200">
    <!-- File Info -->
    <div class="p-4">
      <div class="flex items-start gap-4">
        <!-- File Icon -->
        <div class="w-10 h-10 bg-gradient-to-br from-amber-100 to-orange-100 rounded-lg flex items-center justify-center flex-shrink-0">
          <span class="text-lg">📄</span>
        </div>

        <!-- File Details -->
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 mb-1">
            <h3 class="font-medium text-slate-800 truncate">{{ file.filename }}</h3>
            <span :class="['px-2 py-0.5 text-xs rounded-full', statusInfo.color]">
              {{ statusInfo.icon }} {{ statusInfo.text }}
            </span>
          </div>

          <div class="flex items-center gap-4 text-xs text-slate-400">
            <span>切片方式：{{ sliceTypeLabel }}</span>
            <span>创建时间：{{ formatDate(file.created_at) }}</span>
          </div>

          <!-- Error Log -->
          <div v-if="file.status === -1 && file.log" class="mt-2 p-2 bg-red-50 rounded-lg">
            <p class="text-xs text-red-600">{{ file.log }}</p>
          </div>
        </div>

        <!-- Actions -->
        <div class="flex items-center gap-1">
          <button
            v-if="file.status === 1"
            @click="showSlices = true"
            class="p-2 text-slate-400 hover:text-blue-500 hover:bg-blue-50 rounded-lg transition-all"
            title="查看切片"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 10h16M4 14h16M4 18h16" />
            </svg>
          </button>
          <button
            @click="showSettings = true; selectedSliceType = file.slice_type || 'paragraph'"
            class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-all"
            title="修改切片方式"
          >
            <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.065 2.572c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.572 1.065c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.065-2.572c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
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

    <!-- Slice Settings Modal -->
    <Teleport to="body">
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
              class="flex-1 py-3 bg-gradient-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 disabled:opacity-50"
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
  </div>
</template>
