<script setup>
import { ref, computed, watch } from 'vue'
import { api } from '../api'

const props = defineProps({
  file: {
    type: Object,
    required: true
  }
})

const emit = defineEmits(['close'])

const loading = ref(false)
const entries = ref([])
const needsPassword = ref(false)
const password = ref('')
const error = ref('')
const downloading = ref(new Set())
const expandedDirs = ref(new Set())

const isArchive = computed(() => {
  if (!props.file?.filename) return false
  const lower = props.file.filename.toLowerCase()
  return /\.(zip|7z|tar|tgz|tar\.gz|tar\.bz2|tar\.xz)$/i.test(lower)
})

// 构建文件树
const fileTree = computed(() => {
  const root = { name: '', children: {}, isDir: true, path: '' }

  for (const entry of entries.value) {
    const parts = entry.entry_path.split('/')
    let current = root
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i]
      if (!part) continue
      const isLast = i === parts.length - 1
      const isDir = isLast ? entry.is_directory : true
      const fullPath = parts.slice(0, i + 1).join('/')

      if (!current.children[part]) {
        current.children[part] = {
          name: part,
          children: {},
          isDir,
          path: fullPath,
          size: isLast && !isDir ? entry.size : null,
          entry
        }
      }
      current = current.children[part]
    }
  }

  return root
})

const sortedChildren = (node) => {
  const children = Object.values(node.children || {})
  return children.sort((a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
    return a.name.localeCompare(b.name)
  })
}

const isExpanded = (path) => expandedDirs.value.has(path)

const toggleDir = (path) => {
  if (expandedDirs.value.has(path)) {
    expandedDirs.value.delete(path)
  } else {
    expandedDirs.value.add(path)
  }
}

const formatSize = (bytes) => {
  if (bytes == null) return '-'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
}

const loadEntries = async () => {
  loading.value = true
  error.value = ''
  try {
    const data = await api.getArchiveEntries(props.file.id)
    entries.value = data
    needsPassword.value = false
    // 如果数据库中没有记录（还没解压过），自动解压
    if (entries.value.length === 0) {
      await extractArchive()
    }
  } catch (e) {
    error.value = e.message || '加载失败'
  } finally {
    loading.value = false
  }
}

const extractArchive = async () => {
  loading.value = true
  error.value = ''
  try {
    const result = await api.extractArchive(props.file.id, password.value || null)
    if (result.needs_password) {
      needsPassword.value = true
      entries.value = []
    } else {
      needsPassword.value = false
      entries.value = result.entries || []
      // 默认展开第一层目录
      for (const entry of entries.value) {
        const parts = entry.entry_path.split('/')
        if (parts.length > 1) {
          expandedDirs.value.add(parts[0])
        }
      }
    }
  } catch (e) {
    error.value = e.message || '解压失败'
  } finally {
    loading.value = false
  }
}

const handlePasswordSubmit = async () => {
  if (!password.value.trim()) {
    error.value = '请输入密码'
    return
  }
  await extractArchive()
}

const downloadEntry = async (path) => {
  if (downloading.value.has(path)) return
  downloading.value.add(path)
  try {
    const blob = await api.downloadArchiveEntry(props.file.id, path)
    const url = window.URL.createObjectURL(blob)
    const link = document.createElement('a')
    const filename = path.split('/').pop() || 'download'
    link.href = url
    link.download = filename
    document.body.appendChild(link)
    link.click()
    link.remove()
    window.URL.revokeObjectURL(url)
  } catch (e) {
    alert('下载失败：' + (e?.message || '未知错误'))
  } finally {
    downloading.value.delete(path)
  }
}

watch(() => props.file, () => {
  if (isArchive.value) {
    loadEntries()
  }
}, { immediate: true })
</script>

<template>
  <Teleport to="body">
    <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div @click="emit('close')" class="absolute inset-0 bg-black/50 backdrop-blur-sm"></div>
      <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-2xl max-h-[80vh] flex flex-col">
        <!-- Header -->
        <div class="flex items-center justify-between p-5 border-b border-slate-100">
          <div class="flex items-center gap-3">
            <span class="text-2xl">📦</span>
            <div>
              <h3 class="text-lg font-semibold text-slate-800">压缩文件内容</h3>
              <p class="text-xs text-slate-400">{{ file.filename }}</p>
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

        <!-- Content -->
        <div class="flex-1 overflow-auto p-4">
          <!-- Loading -->
          <div v-if="loading" class="flex items-center justify-center py-12">
            <div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full mr-3"></div>
            <span class="text-slate-500">{{ needsPassword ? '正在解压...' : '正在加载...' }}</span>
          </div>

          <!-- Password Input -->
          <div v-else-if="needsPassword" class="py-8 px-4">
            <div class="text-center mb-6">
              <span class="text-4xl mb-3 block">🔐</span>
              <h4 class="text-lg font-medium text-slate-700 mb-1">压缩文件已加密</h4>
              <p class="text-sm text-slate-400">请输入解压密码</p>
            </div>
            <div class="max-w-xs mx-auto">
              <input
                v-model="password"
                @keyup.enter="handlePasswordSubmit"
                type="password"
                placeholder="输入密码..."
                class="w-full px-4 py-2.5 border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 mb-3"
              />
              <button
                @click="handlePasswordSubmit"
                class="w-full py-2.5 bg-blue-500 text-white rounded-xl font-medium hover:bg-blue-600 transition-all"
              >
                确认解压
              </button>
            </div>
          </div>

          <!-- Error -->
          <div v-else-if="error" class="py-8 text-center">
            <span class="text-3xl mb-2 block">⚠️</span>
            <p class="text-red-500 text-sm">{{ error }}</p>
            <button
              @click="extractArchive"
              class="mt-4 px-4 py-2 bg-slate-100 text-slate-600 rounded-lg hover:bg-slate-200 transition-all text-sm"
            >
              重试
            </button>
          </div>

          <!-- File Tree -->
          <div v-else-if="entries.length > 0">
            <div class="mb-2 px-2 text-xs text-slate-400 flex items-center justify-between">
              <span>共 {{ entries.filter(e => !e.is_directory).length }} 个文件</span>
              <button
                @click="extractArchive"
                class="text-blue-500 hover:text-blue-600 transition-all"
              >
                🔄 重新解压
              </button>
            </div>
            <div class="border border-slate-100 rounded-xl overflow-hidden">
              <!-- 递归渲染树 -->
              <ArchiveTreeNode
                :node="fileTree"
                :level="0"
                :expanded-dirs="expandedDirs"
                :downloading="downloading"
                @toggle="toggleDir"
                @download="downloadEntry"
              />
            </div>
          </div>

          <!-- Empty -->
          <div v-else class="py-12 text-center">
            <span class="text-3xl mb-2 block">📂</span>
            <p class="text-slate-400 text-sm">压缩包为空</p>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script>
// 递归树节点组件（必须在同一文件中定义，因为前端没有单独注册）
export default {
  components: {
    ArchiveTreeNode: {
      name: 'ArchiveTreeNode',
      props: {
        node: { type: Object, required: true },
        level: { type: Number, default: 0 },
        expandedDirs: { type: Set, required: true },
        downloading: { type: Set, required: true }
      },
      emits: ['toggle', 'download'],
      setup(props, { emit, slots }) {
        const isExpanded = (path) => props.expandedDirs.has(path)

        const sortedChildren = (node) => {
          const children = Object.values(node.children || {})
          return children.sort((a, b) => {
            if (a.isDir !== b.isDir) return a.isDir ? -1 : 1
            return a.name.localeCompare(b.name)
          })
        }

        const formatSize = (bytes) => {
          if (bytes == null) return '-'
          if (bytes < 1024) return bytes + ' B'
          if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
          if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
          return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
        }

        return { isExpanded, sortedChildren, formatSize, emit }
      },
      template: `
        <div>
          <div v-for="child in sortedChildren(node)" :key="child.path">
            <div
              v-if="child.isDir"
              class="flex items-center gap-2 py-1.5 px-2 hover:bg-slate-50 rounded cursor-pointer select-none"
              :style="{ paddingLeft: (level * 16 + 8) + 'px' }"
              @click="$emit('toggle', child.path)"
            >
              <span class="text-amber-500 text-sm">{{ isExpanded(child.path) ? '📂' : '📁' }}</span>
              <span class="text-sm text-slate-700">{{ child.name }}</span>
            </div>
            <div
              v-else
              class="flex items-center gap-2 py-1.5 px-2 hover:bg-slate-50 rounded cursor-pointer group"
              :style="{ paddingLeft: (level * 16 + 8) + 'px' }"
            >
              <span class="text-slate-400 text-sm">📄</span>
              <span class="text-sm text-slate-700 flex-1 truncate">{{ child.name }}</span>
              <span class="text-xs text-slate-400 mr-2">{{ formatSize(child.size) }}</span>
              <button
                @click.stop="$emit('download', child.path)"
                :disabled="downloading.has(child.path)"
                class="opacity-0 group-hover:opacity-100 p-1 text-slate-400 hover:text-emerald-500 hover:bg-emerald-50 rounded transition-all text-xs"
                title="下载"
              >
                {{ downloading.has(child.path) ? '⏳' : '⬇️' }}
              </button>
            </div>
            <ArchiveTreeNode
              v-if="child.isDir && isExpanded(child.path)"
              :node="child"
              :level="level + 1"
              :expanded-dirs="expandedDirs"
              :downloading="downloading"
              @toggle="$emit('toggle', $event)"
              @download="$emit('download', $event)"
            />
          </div>
        </div>
      `
    }
  }
}
</script>
