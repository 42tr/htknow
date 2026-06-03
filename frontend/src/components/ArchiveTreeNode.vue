<script setup>
const props = defineProps({
  node: { type: Object, required: true },
  level: { type: Number, default: 0 },
  expandedDirs: { type: Object, required: true },
  downloading: { type: Object, required: true }
})

const emit = defineEmits(['toggle', 'download'])

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
</script>

<template>
  <div>
    <div
      v-for="child in sortedChildren(node)"
      :key="child.path"
    >
      <!-- 目录 -->
      <div
        v-if="child.isDir"
        class="flex items-center gap-2 py-1.5 px-2 hover:bg-slate-50 rounded cursor-pointer select-none"
        :style="{ paddingLeft: (level * 16 + 8) + 'px' }"
        @click="$emit('toggle', child.path)"
      >
        <span class="text-amber-500 text-sm">{{ isExpanded(child.path) ? '📂' : '📁' }}</span>
        <span class="text-sm text-slate-700">{{ child.name }}</span>
      </div>

      <!-- 文件 -->
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

      <!-- 递归渲染子目录 -->
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
</template>
