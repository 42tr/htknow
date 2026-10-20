<script setup>
import { computed, onMounted, ref, watch } from 'vue'
import { getWikiRevision, listWikiRevisions, revertWikiPage } from '@/apis/knowledge/knowledgeApi'
import { diffLines, foldDiff, summarizeDiff } from '@/utils/wikiDiff'

const props = defineProps({
  kbId: { type: [Number, String], required: true },
  slug: { type: String, required: true },
  title: { type: String, default: '' },
  currentVersion: { type: Number, default: 0 },
  currentContent: { type: String, default: '' },
  canEdit: { type: Boolean, default: false },
})
const emit = defineEmits(['close', 'reverted'])

const EDIT_SOURCE_LABELS = { pipeline: '自动生成', user: '人工编辑', revert: '回滚' }
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
    const data = await listWikiRevisions(props.kbId, props.slug, 100)
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
    const data = await getWikiRevision(props.kbId, props.slug, item.version)
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
    const detail = await revertWikiPage(props.kbId, props.slug, version)
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
  <div class="fixed inset-0 z-[1200] flex items-center justify-end bg-black/30" @click.self="emit('close')">
    <aside class="flex h-full w-[480px] max-w-[90vw] flex-col border-l border-cm-divider bg-cm-panel shadow-2xl" role="complementary" :aria-label="`历史版本：${title}`">
      <header class="flex items-center justify-between border-b border-cm-divider px-5 py-4">
        <div>
          <h4 class="m-0 text-base font-semibold text-cm-heading">历史版本</h4>
          <p class="m-0 mt-1 text-xs text-cm-tertiary">{{ title }} · 当前 v{{ currentVersion }}</p>
        </div>
        <button class="h-8 w-8 cursor-pointer rounded-lg border-0 bg-transparent text-cm-secondary transition hover:bg-cm-surface hover:text-cm-heading" aria-label="关闭历史版本" @click="emit('close')">✕</button>
      </header>

      <p v-if="error" class="mx-5 mt-3 rounded-md border border-cm-danger/30 bg-cm-danger/10 px-3 py-2 text-sm text-cm-danger">{{ error }}</p>
      <p v-if="notice" class="mx-5 mt-3 rounded-md border border-cm-success/30 bg-cm-success/10 px-3 py-2 text-sm text-cm-success">{{ notice }}</p>

      <div class="min-h-0 flex-1 overflow-auto px-5 py-3">
        <p v-if="loading" class="text-sm text-cm-tertiary">正在加载历史版本…</p>
        <ul v-else class="m-0 list-none p-0">
          <li v-for="item in revisions" :key="item.id">
            <button
              class="flex w-full cursor-pointer flex-col gap-1 rounded-lg border border-transparent px-3 py-2.5 text-left transition hover:border-cm-default hover:bg-cm-surface"
              :class="{ 'border-cm-focus bg-cm-accent-soft': selected?.version === item.version }"
              @click="select(item)"
            >
              <span class="text-sm font-semibold text-cm-heading">v{{ item.version }}</span>
              <span class="text-xs text-cm-tertiary">
                {{ sourceLabel(item.edit_source) }}
                <template v-if="item.editor_id"> · {{ item.editor_id }}</template>
                · {{ formatTime(item.created_at) }} · {{ item.content_length }} 字
              </span>
            </button>
          </li>
          <li v-if="!revisions.length" class="py-4 text-center text-sm text-cm-tertiary">
            还没有历史版本。页面内容第一次被改写时，旧版本会自动留档在这里。
          </li>
        </ul>
      </div>

      <section v-if="selected" class="flex flex-col border-t border-cm-divider">
        <div class="flex items-center justify-between gap-3 px-5 py-3">
          <span class="text-xs font-medium text-cm-accent">v{{ selected.version }} → 当前 v{{ currentVersion }}</span>
          <span class="rounded-md bg-cm-surface-muted px-2 py-0.5 text-xs text-cm-secondary">+{{ diff.stat.added }} / −{{ diff.stat.removed }}</span>
          <button v-if="canEdit" class="cursor-pointer rounded-lg border border-cm-focus bg-cm-accent px-3 py-1.5 text-xs text-cm-on-accent transition hover:bg-cm-brand-secondary disabled:cursor-not-allowed disabled:opacity-55" :disabled="reverting || diff.identical" @click="revert">
            {{ reverting ? '回滚中…' : '回滚到此版本' }}
          </button>
        </div>
        <p v-if="loadingSelected" class="px-5 pb-4 text-sm text-cm-tertiary">正在加载版本内容…</p>
        <p v-else-if="diff.identical" class="px-5 pb-4 text-sm text-cm-tertiary">与当前内容一致，无需回滚。</p>
        <pre v-else class="max-h-[320px] overflow-auto px-5 pb-4 font-mono text-xs leading-relaxed"><span
          v-for="(line, index) in diff.rows"
          :key="index"
          class="block whitespace-pre-wrap"
          :class="{
            'text-cm-success bg-cm-success/8': line.type === 'insert',
            'text-cm-danger bg-cm-danger/8': line.type === 'delete',
            'text-cm-tertiary': line.type === 'equal',
            'text-center text-cm-tertiary/60': line.type === 'gap',
          }"
        ><template v-if="line.type === 'gap'">⋯ 省略 {{ line.skipped }} 行</template><template
          v-else
        ><span class="inline-block w-6 text-right text-cm-tertiary/50 select-none">{{ line.oldNo ?? '' }}</span><span class="inline-block w-6 text-right text-cm-tertiary/50 select-none">{{ line.newNo ?? '' }}</span> {{ marker(line.type) }} {{ line.value }}</template></span></pre>
      </section>
    </aside>
  </div>
</template>
