<script setup>
import { computed } from 'vue'

defineOptions({ name: 'KnowledgeBaseExportTreeNode' })

const props = defineProps({
  node: { type: Object, required: true },
  selected: { type: Object, required: true },
  depth: { type: Number, default: 0 },
})

const emit = defineEmits(['toggle'])
const children = computed(() => props.node.children || [])
</script>

<template>
  <div class="tree-row" :style="{ '--depth': depth }">
    <input :id="`export-kb-${node.id}`" type="checkbox" :checked="selected.has(node.id)" class="h-4 w-4 rounded border-slate-300" @change="emit('toggle', node)" />
    <label :for="`export-kb-${node.id}`" class="min-w-0 flex-1 cursor-pointer truncate text-sm text-slate-700">{{ node.name }}</label>
    <span class="text-xs text-slate-400">{{ node.file_count || 0 }}</span>
  </div>
  <KnowledgeBaseExportTreeNode v-for="child in children" :key="child.id" :node="child" :selected="selected" :depth="depth + 1" @toggle="emit('toggle', $event)" />
</template>

<style scoped>
.tree-row {
  display: flex;
  align-items: center;
  gap: 0.65rem;
  min-height: 2.5rem;
  padding: 0.35rem 0.5rem 0.35rem calc(0.5rem + var(--depth) * 1.25rem);
  border-radius: 0.5rem;
}

.tree-row:hover { background: #f8f8f8; }
</style>
