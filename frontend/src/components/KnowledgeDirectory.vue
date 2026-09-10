<script setup>
import { ref, onMounted } from 'vue'
import { api } from '../api'
import ResourceIcon from './ResourceIcon.vue'
const props = defineProps({
  parentId: { default: null },
  selectedId: { default: null },
  depth: { default: 0 },
})
const emit = defineEmits(['select', 'create', 'reparse', 'graph'])
const items = ref([]),
  expanded = ref({}),
  error = ref(''),
  loading = ref(false),
  total = ref(0)
let page = 0
const runAction = (event, action, kb) => {
  event.currentTarget.closest('details')?.removeAttribute('open')
  emit(action, kb)
}
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
          v-if="kb.children_kb_count > 0"
          class="directory-expand"
          :aria-label="`${expanded[kb.id] ? '收起' : '展开'} ${kb.name}`"
          :aria-expanded="!!expanded[kb.id]"
          @click="expanded[kb.id] = !expanded[kb.id]"
        >
          <svg
            aria-hidden="true"
            viewBox="0 0 20 20"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            :class="{ expanded: expanded[kb.id] }"
          >
            <path d="m7 5 5 5-5 5" />
          </svg></button
        ><span v-else class="directory-expand-placeholder" aria-hidden="true"></span
        ><button
          class="directory-name"
          :title="kb.name"
          @click="emit('select', kb.id)"
        >
          <ResourceIcon
            type="folder"
            :open="!!expanded[kb.id]"
            class="directory-resource-icon"
          />
          <span class="directory-label">{{ kb.name }}</span>
        </button>
        <details class="directory-actions" @click.stop>
          <summary :aria-label="`${kb.name} 的更多操作`" title="更多操作">
            ...
          </summary>
          <div class="directory-action-menu">
            <button type="button" @click="runAction($event, 'graph', kb)">
              知识图谱
            </button>
            <button type="button" @click="runAction($event, 'create', kb)">
              新建知识库
            </button>
            <button
              type="button"
              :disabled="kb.kb_type === 'storage'"
              :title="
                kb.kb_type === 'storage' ? '存储型知识库不参与解析' : undefined
              "
              @click="runAction($event, 'reparse', kb)"
            >
              重新解析
            </button>
          </div>
        </details>
      </div>
      <KnowledgeDirectory
        v-if="expanded[kb.id]"
        :parent-id="kb.id"
        :selected-id="selectedId"
        :depth="depth + 1"
        @select="emit('select', $event)"
        @create="emit('create', $event)"
        @reparse="emit('reparse', $event)"
        @graph="emit('graph', $event)"
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
  </div>
</template>
