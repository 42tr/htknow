<script setup>
import { computed, ref, watch, onBeforeUnmount } from 'vue'
import { api } from '../api'
import GraphVisualization from './GraphVisualization.vue'
import FileSlices from './FileSlices.vue'
const props = defineProps({
  file: { type: Object, required: true },
  sliceId: { default: null },
})
const emit = defineEmits(['close'])
const tab = ref('preview')
const slices = ref([])
const selected = ref(null)
const loading = ref(false)
const error = ref('')
const blobUrl = ref('')
const editing = ref(false)
let request = 0
const filename = computed(() => props.file.filename || '未命名文件')
const office = computed(() =>
  /\.(pdf|docx?|pptx?|xlsx?)$/i.test(filename.value),
)
const image = computed(() =>
  /\.(png|jpe?g|gif|webp|bmp)$/i.test(filename.value),
)
const viewer = computed(() => {
  if (!office.value || selected.value == null) return ''
  const page = /\.xlsx?$/i.test(filename.value)
    ? 'excel-viewer'
    : 'pdf-highlight'
  return `/${page}.html?${new URLSearchParams({ file_id: props.file.id, slice_id: selected.value })}`
})
const release = () => {
  if (blobUrl.value) URL.revokeObjectURL(blobUrl.value)
  blobUrl.value = ''
}
const load = async () => {
  const id = ++request
  release()
  error.value = ''
  slices.value = []
  selected.value = props.sliceId
  loading.value = true
  try {
    if (image.value) {
      const blob = await api.downloadFile(props.file.id)
      if (id === request) blobUrl.value = URL.createObjectURL(blob)
    } else if (props.file.status === 1 || props.sliceId != null) {
      const data = await api.getFileSlices(props.file.id)
      if (id === request) {
        slices.value = data
        selected.value = props.sliceId ?? data[0]?.id ?? null
      }
    }
  } catch (e) {
    if (id === request) error.value = e.message
  } finally {
    if (id === request) loading.value = false
  }
}
watch(
  () => [props.file.id, props.sliceId],
  () => {
    tab.value = 'preview'
    load()
  },
  { immediate: true },
)
const download = async () => {
  try {
    const blob = await api.downloadFile(props.file.id)
    const url = URL.createObjectURL(blob)
    const link = document.createElement('a')
    link.href = url
    link.download = filename.value
    link.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  } catch (e) {
    error.value = e.message
  }
}
onBeforeUnmount(() => {
  request++
  release()
})
</script>
<template>
  <aside class="file-detail" aria-label="文件详情">
    <div class="panel-heading">
      <div>
        <span class="eyebrow">DOCUMENT</span>
        <h2 :title="filename">{{ filename }}</h2>
      </div>
      <button
        class="plain-button"
        @click="emit('close')"
        aria-label="关闭文件详情"
      >
        ✕
      </button>
    </div>
    <div class="view-tabs detail-tabs">
      <button
        v-for="item in [
          { id: 'preview', name: '原文' },
          { id: 'content', name: '内容段落' },
          { id: 'graph', name: '关联知识' },
          { id: 'info', name: '文件信息' },
        ]"
        :key="item.id"
        :class="{ active: tab === item.id }"
        @click="tab = item.id"
      >
        {{ item.name }}
      </button>
    </div>
    <div v-if="error" role="alert" class="inline-error">
      {{ error }} <button @click="load">重试</button>
    </div>
    <div v-if="loading" class="empty-state" role="status">正在加载文档…</div>
    <template v-else-if="tab === 'preview'">
      <div class="preview-toolbar">
        <select v-if="slices.length" v-model="selected" aria-label="定位段落">
          <option
            v-for="(slice, index) in slices"
            :key="slice.id"
            :value="slice.id"
          >
            段落 {{ index + 1 }}
          </option></select
        ><a v-if="viewer" :href="viewer" target="_blank" rel="noopener"
          >独立窗口打开 ↗</a
        ><button @click="download">下载</button>
      </div>
      <iframe
        v-if="viewer"
        :key="viewer"
        :src="viewer"
        title="文档原文预览"
        class="document-frame"
      ></iframe>
      <img
        v-else-if="image && blobUrl"
        :src="blobUrl"
        :alt="filename"
        class="document-image"
      />
      <div v-else class="empty-state">
        <h3>暂不支持原文预览</h3>
        <p>可查看已解析的内容段落，或下载原文件。</p>
        <button class="secondary-button" @click="tab = 'content'">
          查看内容段落
        </button>
      </div>
    </template>
    <div v-else-if="tab === 'content'" class="detail-content">
      <div class="preview-toolbar">
        <span>{{ slices.length }} 个段落</span
        ><button v-if="slices.length" @click="editing = true">编辑段落</button>
      </div>
      <div v-if="!slices.length" class="empty-state">
        暂无解析内容，请查看文件处理状态。
      </div>
      <article
        v-for="(slice, index) in slices"
        :key="slice.id"
        class="slice-entry"
      >
        <button
          @click="
            () => {
              selected = slice.id
              tab = 'preview'
            }
          "
        >
          段落 {{ index + 1 }} <span v-if="office">· 定位原文 ↗</span>
        </button>
        <p>{{ slice.content }}</p>
      </article>
    </div>
    <div v-else-if="tab === 'graph'" class="detail-content">
      <GraphVisualization v-if="file.status === 1" :file-id="file.id" />
      <div v-else class="empty-state">文件处理完成后可查看关联知识。</div>
    </div>
    <dl v-else class="file-metadata">
      <dt>文件名</dt>
      <dd>{{ filename }}</dd>
      <dt>处理状态</dt>
      <dd>
        {{
          {
            '-1': '处理失败',
            0: '等待处理',
            1: '已完成',
            2: '处理中',
            3: '不解析',
          }[file.status] || '未知状态'
        }}
      </dd>
      <dt>可见性</dt>
      <dd>{{ file.is_public ? '公开' : '私有' }}</dd>
      <dt>切片方式</dt>
      <dd>{{ file.slice_type || '—' }}</dd>
      <dt>上传时间</dt>
      <dd>
        {{
          file.created_at
            ? new Date(file.created_at * 1000).toLocaleString('zh-CN')
            : '—'
        }}
      </dd>
      <template v-if="file.log"
        ><dt>处理记录</dt>
        <dd class="break-all">{{ file.log }}</dd></template
      >
    </dl>
    <FileSlices
      v-if="editing"
      :file="file"
      @close="
        () => {
          editing = false
          load()
        }
      "
    />
  </aside>
</template>
