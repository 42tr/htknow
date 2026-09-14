<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { api } from '../api.js'
import { diffLines, foldDiff, summarizeDiff } from '../wikiDiff.js'

const props = defineProps({
  kbId: { type: [Number, String], required: true },
  slug: { type: String, required: true },
  title: { type: String, default: '' },
  currentVersion: { type: Number, default: 0 },
  currentContent: { type: String, default: '' },
  canEdit: { type: Boolean, default: false },
})
const emit = defineEmits(['close', 'reverted'])

// 版本作者标签：管道生成的版本可以被裁剪，人工版本长期保留。
const EDIT_SOURCE_LABELS = { pipeline: '自动生成', user: '人工编辑', revert: '回滚' }
// diff 里每个变化点上下保留的行数。
const DIFF_CONTEXT = 3

const revisions = ref([])
const loading = ref(false)
const loadingSelected = ref(false)
const reverting = ref(false)
const error = ref('')
const notice = ref('')
const selected = ref(null)

const sourceLabel = (source) => EDIT_SOURCE_LABELS[source] || source || '未知'

const formatTime = (seconds) => (seconds ? new Date(seconds * 1000).toLocaleString('zh-CN') : '')

const marker = (type) => (type === 'insert' ? '+' : type === 'delete' ? '−' : ' ')

// 方向固定为「历史版本 → 当前版本」：加号是当前有、那一版没有的内容。
const diff = computed(() => {
  if (!selected.value) return { rows: [], stat: { added: 0, removed: 0, equal: 0 }, identical: true }
  const rows = diffLines(selected.value.content, props.currentContent)
  return {
    rows: foldDiff(rows, DIFF_CONTEXT),
    stat: summarizeDiff(rows),
    identical: rows.every((row) => row.type === 'equal'),
  }
})

const load = async () => {
  loading.value = true
  error.value = ''
  try {
    const data = await api.listWikiRevisions(props.kbId, props.slug, 100)
    revisions.value = data.items || []
  } catch (e) {
    revisions.value = []
    error.value = e?.message || '加载历史版本失败'
  } finally {
    loading.value = false
  }
}

const select = async (item) => {
  if (selected.value?.version === item.version) {
    selected.value = null
    return
  }
  loadingSelected.value = true
  error.value = ''
  try {
    // 后端一并返回当前正文，省一次页面请求。
    const data = await api.getWikiRevision(props.kbId, props.slug, item.version)
    selected.value = data.revision
  } catch (e) {
    selected.value = null
    error.value = e?.message || '加载版本内容失败'
  } finally {
    loadingSelected.value = false
  }
}

const revert = async () => {
  if (!selected.value) return
  const version = selected.value.version
  if (!window.confirm(`回滚到 v${version}？当前内容会先存成新的历史版本，仍可再回滚回来。`)) return
  reverting.value = true
  error.value = ''
  try {
    const detail = await api.revertWikiPage(props.kbId, props.slug, version)
    selected.value = null
    notice.value = `已回滚到 v${version}，当前为 v${detail.page.version}`
    emit('reverted', detail)
    await load()
  } catch (e) {
    error.value = e?.message || '回滚失败'
  } finally {
    reverting.value = false
  }
}

onMounted(load)

watch(
  () => props.slug,
  () => {
    selected.value = null
    notice.value = ''
    load()
  },
)
</script>

<template>
  <div class="wiki-drawer-backdrop" @click.self="emit('close')">
    <aside class="wiki-drawer" role="complementary" :aria-label="`历史版本：${title}`">
      <header class="wiki-drawer-head">
        <div>
          <h4>历史版本</h4>
          <p class="wiki-drawer-sub">{{ title }} · 当前 v{{ currentVersion }}</p>
        </div>
        <button class="plain-button" aria-label="关闭历史版本" @click="emit('close')">✕</button>
      </header>

      <p v-if="error" class="inline-error wiki-error">{{ error }}</p>
      <p v-if="notice" class="wiki-notice">{{ notice }}</p>

      <div class="wiki-drawer-list">
        <p v-if="loading" class="wiki-empty-line">正在加载历史版本…</p>
        <ul v-else class="wiki-revisions">
          <li v-for="item in revisions" :key="item.id">
            <button class="wiki-revision" :class="{ active: selected?.version === item.version }" @click="select(item)">
              <span class="wiki-revision-version">v{{ item.version }}</span>
              <span class="wiki-revision-meta">
                {{ sourceLabel(item.edit_source) }}
                <template v-if="item.editor_id"> · {{ item.editor_id }}</template>
                · {{ formatTime(item.created_at) }} · {{ item.content_length }} 字
              </span>
            </button>
          </li>
          <li v-if="!revisions.length" class="wiki-empty-line">
            还没有历史版本。页面内容第一次被改写时，旧版本会自动留档在这里。
          </li>
        </ul>
      </div>

      <section v-if="selected" class="wiki-diff">
        <div class="wiki-diff-head">
          <span class="wiki-diff-range">v{{ selected.version }} → 当前 v{{ currentVersion }}</span>
          <span class="wiki-diff-stat">+{{ diff.stat.added }} / −{{ diff.stat.removed }}</span>
          <button v-if="canEdit" class="secondary-button" :disabled="reverting || diff.identical" @click="revert">
            {{ reverting ? '回滚中…' : '回滚到此版本' }}
          </button>
        </div>
        <p v-if="loadingSelected" class="wiki-empty-line">正在加载版本内容…</p>
        <p v-else-if="diff.identical" class="wiki-empty-line">与当前内容一致，无需回滚。</p>
        <pre v-else class="wiki-diff-body"><span
          v-for="(line, index) in diff.rows"
          :key="index"
          class="wiki-diff-line"
          :class="line.type"
        ><template v-if="line.type === 'gap'">⋯ 省略 {{ line.skipped }} 行</template><template
          v-else
        ><i class="wiki-diff-no">{{ line.oldNo ?? '' }}</i><i class="wiki-diff-no">{{ line.newNo ?? '' }}</i>{{ marker(line.type) }} {{ line.value }}</template></span></pre>
      </section>
    </aside>
  </div>
</template>
