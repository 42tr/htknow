<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'
const props = defineProps({
  parentId: { default: null },
  selectedId: { default: null },
  depth: { default: 0 },
})
const emit = defineEmits(['select'])
const items = ref([]),
  expanded = ref({}),
  error = ref(''),
  loading = ref(false),
  total = ref(0)
let page = 0
const load = async () => {
  if (loading.value) return
  loading.value = true
  error.value = ''
  try {
    const data = await api.getKnowledgeBases(props.parentId, {
      page: page + 1,
      size: 50,
    })
    items.value.push(...(data.items || []))
    total.value = data.total || 0
    page++
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}
onMounted(load)
</script>
<template>
  <div class="directory-branch">
    <div v-for="kb in items" :key="kb.id">
      <div
        class="directory-row"
        :class="{ active: selectedId === kb.id }"
        :style="{ paddingLeft: `${depth * 12 + 4}px` }"
      >
        <button
          class="directory-expand"
          :aria-label="`${expanded[kb.id] ? '收起' : '展开'} ${kb.name}`"
          :aria-expanded="!!expanded[kb.id]"
          @click="expanded[kb.id] = !expanded[kb.id]"
        >
          {{ expanded[kb.id] ? '⌄' : '›' }}</button
        ><button
          class="directory-name"
          :title="kb.name"
          @click="emit('select', kb.id)"
        >
          <span aria-hidden="true">▱</span> {{ kb.name }}
        </button>
      </div>
      <KnowledgeDirectory
        v-if="expanded[kb.id]"
        :parent-id="kb.id"
        :selected-id="selectedId"
        :depth="depth + 1"
        @select="emit('select', $event)"
      />
    </div>
    <p v-if="error" class="inline-error">
      {{ error }} <button @click="load">重试</button>
    </p>
    <button
      v-if="items.length < total || loading"
      class="plain-button"
      :disabled="loading"
      @click="load"
    >
      {{ loading ? '加载中…' : '加载更多目录' }}
    </button>
    <p v-if="!loading && !error && !items.length" class="directory-empty">
      暂无子知识库
    </p>
  </div>
</template>
