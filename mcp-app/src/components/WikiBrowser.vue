<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { marked } from 'marked'
import DOMPurify from 'dompurify'
import {
  getWikiIndex, getWikiPage, getWikiStatus, getWikiConfig, updateWikiConfig,
  rebuildWiki, searchWikiPages, updateWikiPage, createWikiPage, deleteWikiPage,
  listWikiPages, listWikiRevisions, lintWiki, rebuildWikiLinks,
} from '@/apis/knowledge/knowledgeApi'
import WikiRevisionDrawer from './WikiRevisionDrawer.vue'

const props = defineProps({
  kbId: { type: [Number, String], required: true },
  title: { type: String, default: '知识库 Wiki' },
  initialSlug: { type: String, default: '' },
  subtitle: { type: String, default: '' },
  canEdit: { type: Boolean, default: false },
})
const emit = defineEmits(['close', 'locate-file'])

const INDEX_SLUG = 'index'
const WIKI_LINK = /\[\[([^\][|]+)(?:\|([^\]]*))?\]\]/g
const GRANULARITIES = [
  { value: 'focused', label: '聚焦' },
  { value: 'standard', label: '标准' },
  { value: 'exhaustive', label: '详尽' },
]
const CREATABLE_TYPES = [
  { value: 'concept', label: '概念' },
  { value: 'entity', label: '实体' },
]
const ISSUE_LABELS = {
  broken_link: '断链',
  empty_content: '空正文',
  missing_summary: '缺摘要',
  orphan_page: '孤儿页',
  stale_source: '来源失效',
  duplicate_title: '标题重复',
}
const EDIT_SOURCE_LABELS = { pipeline: '自动生成', user: '人工编辑', revert: '回滚' }

const indexData = ref(null)
const status = ref(null)
const config = ref(null)
const detail = ref(null)
const currentSlug = ref('')
const loadingIndex = ref(false)
const loadingPage = ref(false)
const error = ref('')
const notice = ref('')
const searchQuery = ref('')
const searchResults = ref(null)
const searching = ref(false)
const expanded = ref({})
const showSettings = ref(false)
const savingConfig = ref(false)
const rebuilding = ref(false)
const draft = ref({ enabled: false, granularity: 'standard', max_pages_per_ingest: 20 })
const editing = ref(false)
const savingEdit = ref(false)
const editForm = ref({ title: '', summary: '', content: '', aliases: '' })
const showRevisions = ref(false)
const archivedOnly = ref(false)
const archivedPages = ref([])
const loadingArchived = ref(false)
const showLint = ref(false)
const lint = ref(null)
const lintLoading = ref(false)
const relinking = ref(false)
const showCreate = ref(false)
const creating = ref(false)
const createForm = ref({ title: '', page_type: 'concept', summary: '', content: '' })
let pollTimer = null
let requestSeq = 0

const groups = computed(() => indexData.value?.groups || [])
const intro = computed(() => indexData.value?.intro || '')
const pendingTasks = computed(() => status.value?.pending_tasks || 0)
const building = computed(() => pendingTasks.value > 0 || (status.value?.builds?.running || 0) > 0)
const enabled = computed(() => Boolean(status.value?.enabled))
const hasPages = computed(() => groups.value.some((group) => group.items?.length))
const activeSearch = computed(() => searchQuery.value.trim().length > 0)
const isIndexPage = computed(() => detail.value?.page?.slug === INDEX_SLUG)
const isArchived = computed(() => detail.value?.page?.status === 'archived')
const canEditPage = computed(() => props.canEdit && Boolean(detail.value) && !isIndexPage.value)
const editSourceLabel = computed(() => EDIT_SOURCE_LABELS[detail.value?.page?.last_edit_source] || '')
const lintIssues = computed(() => lint.value?.issues || [])
const lintKinds = computed(() => lint.value?.by_kind || [])

const titleBySlug = computed(() => {
  const map = {}
  for (const group of groups.value) {
    for (const item of group.items || []) map[item.slug] = item.title
  }
  return map
})

const related = computed(() => {
  const page = detail.value?.page
  if (!page) return { outgoing: [], incoming: [] }
  const label = (slug) => titleBySlug.value[slug] || slug
  return {
    outgoing: (page.out_links || []).map((slug) => ({ slug, title: label(slug) })),
    incoming: (page.in_links || []).map((slug) => ({ slug, title: label(slug) })),
  }
})

const bodyHtml = computed(() => {
  const content = detail.value?.content || ''
  if (!content.trim()) return ''
  return DOMPurify.sanitize(marked.parse(rewriteWikiLinks(content), { gfm: true, breaks: true }))
})

const statusText = computed(() => {
  if (!status.value) return ''
  if (building.value) return `索引中 · 待处理 ${pendingTasks.value}`
  if (status.value.builds?.failed) return `${status.value.builds.failed} 个文件生成失败`
  return `${status.value.page_count || 0} 个条目`
})

const rewriteWikiLinks = (source) => {
  let fenced = false
  return source
    .split('\n')
    .map((line) => {
      const trimmed = line.trimStart()
      if (trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
        fenced = !fenced
        return line
      }
      if (fenced) return line
      return line.replace(WIKI_LINK, (_, slug, label) => {
        const target = slug.trim()
        const text = (label || target).trim().replace(/[[\]]/g, '')
        if (!target) return text
        return `[${text}](#wiki/${encodeURIComponent(target)})`
      })
    })
    .join('\n')
}

const flash = (message) => {
  notice.value = message
  window.setTimeout(() => {
    if (notice.value === message) notice.value = ''
  }, 4000)
}

const loadIndex = async () => {
  loadingIndex.value = true
  try {
    indexData.value = await getWikiIndex(props.kbId)
    for (const group of groups.value) expanded.value[group.page_type] = true
  } catch (e) {
    error.value = e?.message || '加载 Wiki 目录失败'
  } finally {
    loadingIndex.value = false
  }
}

const loadStatus = async () => {
  try {
    status.value = await getWikiStatus(props.kbId)
  } catch (e) {
    if (!error.value) error.value = e?.message || '加载 Wiki 状态失败'
  }
}

const loadConfig = async () => {
  try {
    config.value = await getWikiConfig(props.kbId)
    draft.value = {
      enabled: Boolean(config.value.enabled),
      granularity: config.value.granularity || 'standard',
      max_pages_per_ingest: config.value.max_pages_per_ingest || 20,
    }
  } catch (e) {
    config.value = null
  }
}

const openPage = async (slug) => {
  const target = (slug || '').trim()
  if (!target) return
  const request = ++requestSeq
  loadingPage.value = true
  error.value = ''
  try {
    const data = await getWikiPage(props.kbId, target)
    if (request !== requestSeq) return
    detail.value = data
    currentSlug.value = data.page.slug
    searchQuery.value = ''
    searchResults.value = null
    editing.value = false
  } catch (e) {
    if (request !== requestSeq) return
    if (target !== currentSlug.value) detail.value = null
    error.value = e?.message || '加载 Wiki 页面失败'
  } finally {
    if (request === requestSeq) loadingPage.value = false
  }
}

const runSearch = async () => {
  const query = searchQuery.value.trim()
  if (!query) {
    searchResults.value = null
    return
  }
  searching.value = true
  error.value = ''
  try {
    const data = await searchWikiPages(props.kbId, query, 30)
    searchResults.value = data.items || []
  } catch (e) {
    error.value = e?.message || 'Wiki 搜索失败'
  } finally {
    searching.value = false
  }
}

const onBodyClick = (event) => {
  const anchor = event.target.closest('a')
  if (!anchor) return
  const href = anchor.getAttribute('href') || ''
  if (href.startsWith('#wiki/')) {
    event.preventDefault()
    openPage(decodeURIComponent(href.slice('#wiki/'.length)))
    return
  }
  if (/^https?:/i.test(href)) {
    event.preventDefault()
    window.open(href, '_blank', 'noopener')
  }
}

const locateFile = (source) => {
  emit('locate-file', { id: source.file_id, kb_id: props.kbId })
  emit('close')
}

const slicesOf = (fileId) => (detail.value?.slices || []).filter((slice) => slice.file_id === fileId)

const saveConfig = async () => {
  savingConfig.value = true
  error.value = ''
  try {
    config.value = await updateWikiConfig({
      kb_id: props.kbId,
      enabled: draft.value.enabled,
      granularity: draft.value.granularity,
      max_pages_per_ingest: Number(draft.value.max_pages_per_ingest) || 0,
    })
    draft.value.enabled = Boolean(config.value.enabled)
    await Promise.all([loadStatus(), loadIndex()])
    flash('Wiki 配置已保存')
  } catch (e) {
    error.value = e?.message || '保存 Wiki 配置失败'
  } finally {
    savingConfig.value = false
  }
}

const rebuild = async () => {
  if (!window.confirm('将按当前配置为已完成解析的文件重新生成 Wiki，可能产生较多 LLM 调用。继续？')) return
  rebuilding.value = true
  error.value = ''
  try {
    const result = await rebuildWiki(props.kbId)
    flash(`已入队 ${result.enqueued} 个文件，生成中…`)
    await Promise.all([loadStatus(), loadIndex()])
  } catch (e) {
    error.value = e?.message || '触发 Wiki 重建失败'
  } finally {
    rebuilding.value = false
  }
}

const issueLabel = (kind) => ISSUE_LABELS[kind] || kind
const parseAliases = (value) => (value || '').split(/[,，、;；\n]/).map((item) => item.trim()).filter(Boolean)

const startEdit = () => {
  if (!detail.value) return
  editForm.value = {
    title: detail.value.page.title || '',
    summary: detail.value.page.summary || '',
    content: detail.value.content || '',
    aliases: (detail.value.page.aliases || []).join('、'),
  }
  showLint.value = false
  showCreate.value = false
  editing.value = true
}

const saveEdit = async () => {
  if (!editForm.value.title.trim()) {
    error.value = '标题不能为空'
    return
  }
  savingEdit.value = true
  error.value = ''
  try {
    detail.value = await updateWikiPage({
      kb_id: props.kbId,
      slug: currentSlug.value,
      title: editForm.value.title.trim(),
      summary: editForm.value.summary,
      content: editForm.value.content,
      aliases: parseAliases(editForm.value.aliases),
    })
    editing.value = false
    flash(`已保存为 v${detail.value.page.version}`)
    await loadIndex()
  } catch (e) {
    error.value = e?.message || '保存 Wiki 页面失败'
  } finally {
    savingEdit.value = false
  }
}

const changeStatus = async (newStatus) => {
  error.value = ''
  try {
    detail.value = await updateWikiPage({ kb_id: props.kbId, slug: currentSlug.value, status: newStatus })
    flash(newStatus === 'archived' ? '已归档：退出目录与搜索，可在「已归档」中恢复' : '已恢复发布')
    await Promise.all([loadIndex(), loadStatus()])
    if (archivedOnly.value) await loadArchived()
  } catch (e) {
    error.value = e?.message || '更新页面状态失败'
  }
}

const removePage = async () => {
  if (!detail.value) return
  const title = detail.value.page.title || currentSlug.value
  if (!window.confirm(`删除「${title}」？历史版本会一并清除。只是不想让它出现在目录里的话，请用归档。`)) return
  error.value = ''
  try {
    await deleteWikiPage(props.kbId, currentSlug.value)
    detail.value = null
    currentSlug.value = ''
    editing.value = false
    showRevisions.value = false
    flash('页面已删除')
    await Promise.all([loadIndex(), loadStatus()])
    if (archivedOnly.value) await loadArchived()
  } catch (e) {
    error.value = e?.message || '删除 Wiki 页面失败'
  }
}

const loadArchived = async () => {
  loadingArchived.value = true
  try {
    const data = await listWikiPages(props.kbId, { status: 'archived', limit: 200 })
    archivedPages.value = data.items || []
  } catch (e) {
    error.value = e?.message || '加载归档页面失败'
  } finally {
    loadingArchived.value = false
  }
}

const toggleArchived = async () => {
  archivedOnly.value = !archivedOnly.value
  searchQuery.value = ''
  searchResults.value = null
  if (archivedOnly.value) await loadArchived()
}

const loadLint = async () => {
  lintLoading.value = true
  error.value = ''
  try {
    lint.value = await lintWiki(props.kbId)
  } catch (e) {
    lint.value = null
    error.value = e?.message || 'Wiki 体检失败'
  } finally {
    lintLoading.value = false
  }
}

const toggleLint = async () => {
  showLint.value = !showLint.value
  showCreate.value = false
  if (showLint.value) await loadLint()
}

const openIssuePage = async (issue) => {
  showLint.value = false
  await openPage(issue.slug)
}

const rebuildLinks = async () => {
  relinking.value = true
  error.value = ''
  try {
    const report = await rebuildWikiLinks(props.kbId)
    flash(`已收敛 ${report.pages_changed} 个页面，清理 ${report.dead_links_removed} 处死链`)
    await Promise.all([loadLint(), loadIndex()])
    if (currentSlug.value) await openPage(currentSlug.value)
  } catch (e) {
    error.value = e?.message || '重建交叉链接失败'
  } finally {
    relinking.value = false
  }
}

const submitCreate = async () => {
  if (!createForm.value.title.trim()) {
    error.value = '标题不能为空'
    return
  }
  creating.value = true
  error.value = ''
  try {
    const created = await createWikiPage({
      kb_id: props.kbId,
      title: createForm.value.title.trim(),
      page_type: createForm.value.page_type,
      summary: createForm.value.summary,
      content: createForm.value.content,
    })
    showCreate.value = false
    createForm.value = { title: '', page_type: 'concept', summary: '', content: '' }
    archivedOnly.value = false
    await loadIndex()
    await openPage(created.page.slug)
    flash(`已创建 ${created.page.slug}`)
  } catch (e) {
    error.value = e?.message || '新建 Wiki 条目失败'
  } finally {
    creating.value = false
  }
}

const onReverted = async (updated) => {
  detail.value = updated
  await Promise.all([loadIndex(), loadStatus()])
}

const refreshAll = async () => {
  error.value = ''
  await Promise.all([loadIndex(), loadStatus()])
  if (currentSlug.value) await openPage(currentSlug.value)
}

watch(building, (active) => {
  if (active && !pollTimer) {
    pollTimer = window.setInterval(async () => {
      await loadStatus()
      if (!building.value) {
        await loadIndex()
        if (currentSlug.value) await openPage(currentSlug.value)
      }
    }, 5000)
  } else if (!active && pollTimer) {
    window.clearInterval(pollTimer)
    pollTimer = null
  }
})

onMounted(async () => {
  await Promise.all([loadIndex(), loadStatus(), loadConfig()])
  if (props.initialSlug) {
    await openPage(props.initialSlug)
    return
  }
  if (!currentSlug.value) await openPage(INDEX_SLUG)
  if (!detail.value) {
    const first = groups.value.flatMap((group) => group.items || [])[0]
    if (first) await openPage(first.slug)
  }
})

onBeforeUnmount(() => {
  if (pollTimer) window.clearInterval(pollTimer)
  requestSeq++
})
</script>

<template>
  <Teleport to="body">
    <div class="fixed inset-0 z-[1100] flex items-center justify-center bg-black/30" @click.self="emit('close')" @keydown.esc="emit('close')">
      <section class="flex h-[min(92vh,900px)] w-[min(94vw,1200px)] flex-col overflow-hidden rounded-2xl border border-cm-strong bg-cm-panel shadow-2xl" role="dialog" aria-modal="true" :aria-label="title">

        <!-- Header -->
        <div class="flex items-start justify-between border-b border-cm-divider px-6 py-4">
          <div>
            <h2 class="m-0 text-lg font-semibold text-cm-heading">{{ title }}</h2>
            <p class="m-0 mt-1 text-sm text-cm-tertiary">{{ subtitle || '由文档自动生成的知识库条目与交叉链接。' }}</p>
          </div>
          <div class="flex items-center gap-2">
            <span v-if="statusText" class="rounded-md border px-2.5 py-1 text-xs font-medium" :class="building ? 'border-cm-info/40 bg-cm-info/10 text-cm-info' : 'border-cm-default bg-cm-surface-muted text-cm-secondary'">{{ statusText }}</span>
            <button v-if="canEdit" class="cursor-pointer rounded-lg border-0 bg-transparent px-2.5 py-1.5 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="showCreate = !showCreate; showLint = false">
              {{ showCreate ? '收起新建' : '新建条目' }}
            </button>
            <button class="cursor-pointer rounded-lg border-0 bg-transparent px-2.5 py-1.5 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="toggleLint">
              {{ showLint ? '收起体检' : '体检' }}
            </button>
            <button class="cursor-pointer rounded-lg border-0 bg-transparent px-2.5 py-1.5 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="showSettings = !showSettings">
              {{ showSettings ? '收起设置' : '设置' }}
            </button>
            <button class="flex h-8 w-8 cursor-pointer items-center justify-center rounded-lg border-0 bg-transparent text-cm-secondary transition hover:bg-cm-surface hover:text-cm-heading" aria-label="关闭 Wiki" @click="emit('close')">✕</button>
          </div>
        </div>

        <!-- Settings Panel -->
        <div v-if="showSettings" class="border-b border-cm-divider bg-cm-surface-muted px-6 py-4">
          <div class="flex flex-wrap items-center gap-4">
            <label class="flex items-center gap-2 text-sm text-cm-primary">
              <input v-model="draft.enabled" type="checkbox" :disabled="!canEdit" class="accent-[var(--cm-brand-primary)]" />
              为本知识库生成 Wiki
            </label>
            <label class="flex items-center gap-2 text-sm text-cm-primary">
              粒度
              <select v-model="draft.granularity" :disabled="!canEdit" class="rounded-md border border-cm-default bg-cm-input px-2 py-1 text-sm text-cm-primary">
                <option v-for="item in GRANULARITIES" :key="item.value" :value="item.value">{{ item.label }}</option>
              </select>
            </label>
            <label class="flex items-center gap-2 text-sm text-cm-primary">
              单文件条目上限
              <input v-model.number="draft.max_pages_per_ingest" type="number" min="0" max="200" :disabled="!canEdit" class="w-20 rounded-md border border-cm-default bg-cm-input px-2 py-1 text-sm text-cm-primary" />
            </label>
            <div class="flex items-center gap-2">
              <span v-if="config && !config.llm_available" class="text-xs text-cm-warning">未配置 LLM，无法生成内容</span>
              <span v-if="!canEdit" class="text-xs text-cm-warning">需要编辑权限才能修改</span>
              <button class="cursor-pointer rounded-lg border border-cm-focus bg-cm-accent px-3 py-1.5 text-xs text-cm-on-accent transition hover:bg-cm-brand-secondary disabled:cursor-not-allowed disabled:opacity-55" :disabled="!canEdit || savingConfig" @click="saveConfig">
                {{ savingConfig ? '保存中…' : '保存' }}
              </button>
              <button class="cursor-pointer rounded-lg border border-cm-default bg-cm-surface px-3 py-1.5 text-xs text-cm-accent transition hover:border-cm-focus hover:text-cm-heading disabled:cursor-not-allowed disabled:opacity-55" :disabled="!canEdit || rebuilding || !enabled" @click="rebuild">
                {{ rebuilding ? '入队中…' : '重建全库' }}
              </button>
            </div>
          </div>
        </div>

        <!-- Error/Notice -->
        <p v-if="error" class="mx-6 mt-3 rounded-md border border-cm-danger/30 bg-cm-danger/10 px-3 py-2 text-sm text-cm-danger">{{ error }}</p>
        <p v-if="notice" class="mx-6 mt-3 rounded-md border border-cm-success/30 bg-cm-success/10 px-3 py-2 text-sm text-cm-success">{{ notice }}</p>

        <!-- Main Body: Sidebar + Content -->
        <div class="flex min-h-0 flex-1">

          <!-- Sidebar -->
          <aside class="flex w-[260px] flex-none flex-col border-r border-cm-divider bg-cm-surface-muted">
            <div class="flex gap-1.5 px-3 py-3">
              <input v-model="searchQuery" type="search" placeholder="搜索条目…" aria-label="搜索 Wiki 条目" class="min-w-0 flex-1 rounded-md border border-cm-default bg-cm-input px-2.5 py-1.5 text-sm text-cm-primary placeholder:text-cm-tertiary" @keyup.enter="runSearch" />
              <button class="cursor-pointer rounded-md border border-cm-default bg-cm-surface px-2.5 py-1.5 text-xs text-cm-accent transition hover:border-cm-focus hover:text-cm-heading disabled:opacity-55" :disabled="searching" @click="runSearch">搜索</button>
            </div>
            <div class="px-3 pb-2">
              <button class="cursor-pointer rounded-md border-0 bg-transparent px-1 py-0.5 text-xs text-cm-secondary transition hover:text-cm-heading" :class="{ 'text-cm-accent font-medium': archivedOnly }" @click="toggleArchived">
                {{ archivedOnly ? '← 返回目录' : '已归档' }}
              </button>
            </div>

            <div class="min-h-0 flex-1 overflow-auto px-2 pb-3">
              <!-- Search results -->
              <ul v-if="activeSearch && searchResults" class="m-0 list-none p-0">
                <li v-for="item in searchResults" :key="item.slug">
                  <button class="flex w-full cursor-pointer flex-col gap-0.5 rounded-md border border-transparent px-2.5 py-2 text-left transition hover:border-cm-default hover:bg-cm-surface" :class="{ 'border-cm-focus bg-cm-accent-soft': item.slug === currentSlug }" @click="openPage(item.slug)">
                    <span class="text-sm font-medium text-cm-heading">{{ item.title }}</span>
                    <span class="text-xs text-cm-tertiary">{{ item.page_type }}</span>
                  </button>
                  <p v-if="item.summary" class="m-0 mt-0.5 px-2.5 text-xs text-cm-tertiary line-clamp-2">{{ item.summary }}</p>
                </li>
                <li v-if="!searchResults.length" class="py-4 text-center text-xs text-cm-tertiary">没有匹配的条目</li>
              </ul>

              <!-- Archived -->
              <ul v-else-if="archivedOnly" class="m-0 list-none p-0">
                <li v-for="item in archivedPages" :key="item.slug">
                  <button class="flex w-full cursor-pointer items-center gap-2 rounded-md border border-transparent px-2.5 py-2 text-left transition hover:border-cm-default hover:bg-cm-surface" :class="{ 'border-cm-focus bg-cm-accent-soft': item.slug === currentSlug }" @click="openPage(item.slug)">
                    <span class="min-w-0 flex-1 truncate text-sm text-cm-heading">{{ item.title }}</span>
                    <span class="flex-none text-xs text-cm-tertiary">已归档</span>
                  </button>
                </li>
                <li v-if="loadingArchived" class="py-4 text-center text-xs text-cm-tertiary">正在加载归档页面…</li>
                <li v-else-if="!archivedPages.length" class="py-4 text-center text-xs text-cm-tertiary">没有归档页面。</li>
              </ul>

              <!-- Normal tree -->
              <template v-else>
                <button class="mb-1 flex w-full cursor-pointer items-center rounded-md border border-transparent px-2.5 py-2 text-left text-sm font-medium transition hover:border-cm-default hover:bg-cm-surface" :class="{ 'border-cm-focus bg-cm-accent-soft text-cm-heading': currentSlug === INDEX_SLUG, 'text-cm-primary': currentSlug !== INDEX_SLUG }" @click="openPage(INDEX_SLUG)">
                  总览
                </button>
                <div v-for="group in groups" :key="group.page_type" class="mb-1">
                  <button class="flex w-full cursor-pointer items-center justify-between rounded-md border-0 bg-transparent px-2.5 py-1.5 text-xs font-semibold text-cm-secondary transition hover:text-cm-heading" @click="expanded[group.page_type] = !expanded[group.page_type]">
                    <span>{{ group.title }}</span>
                    <span class="rounded bg-cm-surface-muted px-1.5 text-xs">{{ group.count }}</span>
                  </button>
                  <ul v-if="expanded[group.page_type]" class="m-0 list-none p-0">
                    <li v-for="item in group.items" :key="item.slug">
                      <button class="flex w-full cursor-pointer items-center rounded-md border border-transparent px-4 py-1.5 text-left text-sm transition hover:border-cm-default hover:bg-cm-surface" :class="{ 'border-cm-focus bg-cm-accent-soft text-cm-heading': item.slug === currentSlug, 'text-cm-primary': item.slug !== currentSlug }" :title="item.summary" @click="openPage(item.slug)">
                        {{ item.title }}
                      </button>
                    </li>
                  </ul>
                </div>
                <p v-if="loadingIndex" class="py-4 text-center text-xs text-cm-tertiary">正在加载目录…</p>
                <p v-else-if="!hasPages" class="px-3 py-4 text-center text-xs leading-relaxed text-cm-tertiary">
                  还没有 Wiki 条目。在「设置」中开启后，重新解析或点击「重建全库」即可生成。
                </p>
              </template>
            </div>
          </aside>

          <!-- Page Content -->
          <div class="min-h-0 flex-1 overflow-auto px-6 py-5" @click="onBodyClick">

            <!-- Lint Panel -->
            <section v-if="showLint" class="mb-5 rounded-xl border border-cm-default bg-cm-surface p-5">
              <div class="flex items-center justify-between">
                <h3 class="m-0 text-base font-semibold text-cm-heading">体检结果</h3>
                <div class="flex gap-2">
                  <button class="cursor-pointer rounded-lg border border-cm-default bg-cm-surface px-3 py-1.5 text-xs text-cm-accent transition hover:border-cm-focus hover:text-cm-heading disabled:opacity-55" :disabled="lintLoading" @click="loadLint">
                    {{ lintLoading ? '检查中…' : '重新检查' }}
                  </button>
                  <button class="cursor-pointer rounded-lg border border-cm-default bg-cm-surface px-3 py-1.5 text-xs text-cm-accent transition hover:border-cm-focus hover:text-cm-heading disabled:opacity-55" :disabled="relinking || !canEdit" @click="rebuildLinks">
                    {{ relinking ? '收敛中…' : '重建交叉链接' }}
                  </button>
                </div>
              </div>
              <p v-if="lint" class="mt-3 flex flex-wrap items-center gap-2 text-sm text-cm-secondary">
                扫描 {{ lint.scanned }} 个页面，发现 {{ lint.issue_count }} 个问题
                <template v-if="lint.truncated">（仅列出前 {{ lintIssues.length }} 条）</template>
                <span v-for="kind in lintKinds" :key="kind.kind" class="rounded-md border border-cm-default bg-cm-surface-muted px-2 py-0.5 text-xs">
                  {{ issueLabel(kind.kind) }} {{ kind.count }}
                </span>
              </p>
              <ul v-if="lintIssues.length" class="m-0 mt-3 list-none space-y-1 p-0">
                <li v-for="(issue, index) in lintIssues" :key="`${issue.kind}-${issue.page_id}-${index}`" class="flex items-center gap-2 rounded-md border border-cm-default bg-cm-surface px-3 py-2 text-sm" :class="{ 'border-cm-warning/40': issue.severity === 'warning', 'border-cm-danger/40': issue.severity === 'error' }">
                  <button class="min-w-0 flex-1 cursor-pointer truncate border-0 bg-transparent text-left text-cm-accent transition hover:text-cm-heading" @click="openIssuePage(issue)">{{ issue.title || issue.slug }}</button>
                  <span class="flex-none text-xs text-cm-tertiary">{{ issueLabel(issue.kind) }}</span>
                </li>
              </ul>
              <p v-else-if="lint" class="mt-2 text-sm text-cm-success">没有发现明显问题。</p>
              <p v-if="lint?.orphan_count" class="mt-2 text-xs text-cm-tertiary">另有 {{ lint.orphan_count }} 个孤儿页（没有任何链接指向它们）。</p>
            </section>

            <!-- Create Form -->
            <section v-if="showCreate" class="mb-5 rounded-xl border border-cm-default bg-cm-surface p-5">
              <h3 class="m-0 mb-3 text-base font-semibold text-cm-heading">新建条目</h3>
              <div class="space-y-3">
                <label class="block text-sm text-cm-secondary">
                  标题
                  <input v-model="createForm.title" type="text" maxlength="200" required class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 text-sm text-cm-primary" />
                </label>
                <label class="block text-sm text-cm-secondary">
                  类型
                  <select v-model="createForm.page_type" class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 text-sm text-cm-primary">
                    <option v-for="t in CREATABLE_TYPES" :key="t.value" :value="t.value">{{ t.label }}</option>
                  </select>
                </label>
                <label class="block text-sm text-cm-secondary">
                  摘要
                  <input v-model="createForm.summary" type="text" maxlength="2000" placeholder="一句话说明" class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 text-sm text-cm-primary" />
                </label>
                <label class="block text-sm text-cm-secondary">
                  正文（Markdown）
                  <textarea v-model="createForm.content" rows="8" class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 font-mono text-sm text-cm-primary"></textarea>
                </label>
                <div class="flex justify-end gap-2">
                  <button class="cursor-pointer rounded-lg border border-cm-default bg-cm-surface px-3 py-1.5 text-sm text-cm-accent transition hover:border-cm-focus hover:text-cm-heading" @click="showCreate = false">取消</button>
                  <button class="cursor-pointer rounded-lg border border-cm-focus bg-cm-accent px-3 py-1.5 text-sm text-cm-on-accent transition hover:bg-cm-brand-secondary disabled:opacity-55" :disabled="creating" @click="submitCreate">
                    {{ creating ? '创建中…' : '创建' }}
                  </button>
                </div>
              </div>
            </section>

            <!-- Page Detail -->
            <template v-if="detail">
              <header class="mb-4">
                <div class="flex flex-wrap items-center gap-2 text-xs text-cm-tertiary">
                  <span class="rounded-md border border-cm-default bg-cm-surface-muted px-2 py-0.5">{{ detail.page.page_type }}</span>
                  <span v-if="detail.page.status !== 'published'" class="rounded-md border border-cm-warning/40 bg-cm-warning/10 px-2 py-0.5 text-cm-warning">{{ detail.page.status }}</span>
                  <span>v{{ detail.page.version }}</span>
                  <span v-if="editSourceLabel" class="rounded-md border border-cm-default bg-cm-surface-muted px-2 py-0.5">{{ editSourceLabel }}</span>
                </div>
                <h3 class="m-0 mt-2 text-xl font-semibold text-cm-heading">{{ detail.page.title }}</h3>
                <p v-if="detail.page.summary" class="m-0 mt-1 text-sm text-cm-tertiary">{{ detail.page.summary }}</p>
              </header>

              <div class="mb-4 flex flex-wrap items-center gap-2">
                <button class="cursor-pointer rounded-lg border-0 bg-transparent px-2 py-1 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="showRevisions = true">历史版本</button>
                <template v-if="canEditPage">
                  <button v-if="!editing" class="cursor-pointer rounded-lg border-0 bg-transparent px-2 py-1 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="startEdit">编辑</button>
                  <button v-if="isArchived" class="cursor-pointer rounded-lg border-0 bg-transparent px-2 py-1 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="changeStatus('published')">恢复发布</button>
                  <button v-else class="cursor-pointer rounded-lg border-0 bg-transparent px-2 py-1 text-sm text-cm-accent transition hover:bg-cm-surface hover:text-cm-heading" @click="changeStatus('archived')">归档</button>
                  <button class="cursor-pointer rounded-lg border-0 bg-transparent px-2 py-1 text-sm text-cm-danger transition hover:bg-cm-danger/10" @click="removePage">删除</button>
                </template>
                <span v-else-if="isIndexPage" class="text-xs text-cm-tertiary">索引页由系统维护，不可手工编辑。</span>
              </div>

              <!-- Edit Form -->
              <form v-if="editing" class="mb-6 space-y-3 rounded-xl border border-cm-default bg-cm-surface p-5" @submit.prevent="saveEdit">
                <label class="block text-sm text-cm-secondary">
                  标题
                  <input v-model="editForm.title" type="text" maxlength="200" required class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 text-sm text-cm-primary" />
                </label>
                <label class="block text-sm text-cm-secondary">
                  摘要
                  <input v-model="editForm.summary" type="text" maxlength="2000" placeholder="一句话说明，出现在目录与搜索结果里" class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 text-sm text-cm-primary" />
                </label>
                <label class="block text-sm text-cm-secondary">
                  别名
                  <input v-model="editForm.aliases" type="text" placeholder="用顿号或逗号分隔，用于交叉链接匹配" class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 text-sm text-cm-primary" />
                </label>
                <label class="block text-sm text-cm-secondary">
                  正文（Markdown，内链写作 [[slug|显示名]]）
                  <textarea v-model="editForm.content" rows="18" class="mt-1 w-full rounded-md border border-cm-default bg-cm-input px-3 py-2 font-mono text-sm text-cm-primary"></textarea>
                </label>
                <div class="flex items-center justify-between">
                  <span class="text-xs text-cm-warning">保存前会自动留下版本快照，可随时回滚；自动生成的内容不会再覆盖人工编辑。</span>
                  <div class="flex gap-2">
                    <button type="button" class="cursor-pointer rounded-lg border border-cm-default bg-cm-surface px-3 py-1.5 text-sm text-cm-accent transition hover:border-cm-focus hover:text-cm-heading disabled:opacity-55" :disabled="savingEdit" @click="editing = false">取消</button>
                    <button type="submit" class="cursor-pointer rounded-lg border border-cm-focus bg-cm-accent px-3 py-1.5 text-sm text-cm-on-accent transition hover:bg-cm-brand-secondary disabled:opacity-55" :disabled="savingEdit">
                      {{ savingEdit ? '保存中…' : '保存' }}
                    </button>
                  </div>
                </div>
              </form>

              <!-- Rendered Content -->
              <article v-else class="wiki-markdown prose prose-sm max-w-none text-cm-primary" v-html="bodyHtml"></article>

              <!-- Related Links -->
              <section v-if="related.outgoing.length || related.incoming.length" class="mt-6 rounded-xl border border-cm-default bg-cm-surface p-4">
                <div v-if="related.outgoing.length" class="mb-3">
                  <h4 class="m-0 mb-2 text-sm font-semibold text-cm-heading">链接到</h4>
                  <div class="flex flex-wrap gap-1.5">
                    <button v-for="link in related.outgoing" :key="`out-${link.slug}`" class="cursor-pointer rounded-md border border-cm-default bg-cm-surface-muted px-2.5 py-1 text-xs text-cm-accent transition hover:border-cm-focus hover:text-cm-heading" @click="openPage(link.slug)">
                      {{ link.title }}
                    </button>
                  </div>
                </div>
                <div v-if="related.incoming.length">
                  <h4 class="m-0 mb-2 text-sm font-semibold text-cm-heading">被引用</h4>
                  <div class="flex flex-wrap gap-1.5">
                    <button v-for="link in related.incoming" :key="`in-${link.slug}`" class="cursor-pointer rounded-md border border-cm-default bg-cm-surface-muted px-2.5 py-1 text-xs text-cm-accent transition hover:border-cm-focus hover:text-cm-heading" @click="openPage(link.slug)">
                      {{ link.title }}
                    </button>
                  </div>
                </div>
              </section>

              <!-- Sources -->
              <section v-if="detail.sources?.length" class="mt-4 rounded-xl border border-cm-default bg-cm-surface p-4">
                <h4 class="m-0 mb-3 text-sm font-semibold text-cm-heading">来源（{{ detail.sources.length }} 个文件 · {{ detail.slices?.length || 0 }} 处证据）</h4>
                <div v-for="source in detail.sources" :key="source.file_id" class="mb-2 last:mb-0">
                  <button class="cursor-pointer border-0 bg-transparent text-left text-sm text-cm-accent transition hover:text-cm-heading hover:underline" :title="source.filename" @click="locateFile(source)">
                    {{ source.filename }}
                  </button>
                  <div v-if="slicesOf(source.file_id).length" class="mt-1 flex flex-wrap gap-1.5">
                    <span v-for="slice in slicesOf(source.file_id)" :key="slice.slice_id" class="rounded-md border border-cm-default bg-cm-surface-muted px-2 py-0.5 text-xs text-cm-secondary">
                      原文 #{{ slice.slice_id }}
                    </span>
                  </div>
                </div>
              </section>
            </template>

            <div v-else-if="!error" class="flex h-full flex-col items-center justify-center text-center">
              <h3 class="m-0 text-lg font-semibold text-cm-heading">还没有内容</h3>
              <p class="m-0 mt-2 text-sm text-cm-tertiary">Wiki 会在文档解析完成后自动生成；也可以在「设置」里手动触发重建。</p>
            </div>
          </div>
        </div>

        <!-- Revision Drawer -->
        <WikiRevisionDrawer
          v-if="showRevisions && detail"
          :kb-id="kbId"
          :slug="detail.page.slug"
          :title="detail.page.title"
          :current-version="detail.page.version"
          :current-content="detail.content"
          :can-edit="canEditPage"
          @close="showRevisions = false"
          @reverted="onReverted"
        />
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.wiki-markdown :deep(h1) { font-size: 1.5em; font-weight: 700; margin: 1.5em 0 0.5em; color: var(--cm-text-heading); }
.wiki-markdown :deep(h2) { font-size: 1.25em; font-weight: 600; margin: 1.3em 0 0.4em; color: var(--cm-text-heading); }
.wiki-markdown :deep(h3) { font-size: 1.1em; font-weight: 600; margin: 1.2em 0 0.3em; color: var(--cm-text-heading); }
.wiki-markdown :deep(p) { margin: 0.6em 0; line-height: 1.7; }
.wiki-markdown :deep(ul), .wiki-markdown :deep(ol) { margin: 0.6em 0; padding-left: 1.5em; }
.wiki-markdown :deep(li) { margin: 0.3em 0; }
.wiki-markdown :deep(a) { color: var(--cm-brand-primary); text-decoration: underline; text-underline-offset: 2px; }
.wiki-markdown :deep(a:hover) { color: var(--cm-brand-secondary); }
.wiki-markdown :deep(code) { background: var(--cm-bg-surface-muted); padding: 0.15em 0.4em; border-radius: 4px; font-size: 0.875em; }
.wiki-markdown :deep(pre) { background: var(--cm-bg-surface-muted); padding: 1em; border-radius: 8px; overflow-x: auto; margin: 1em 0; }
.wiki-markdown :deep(pre code) { background: transparent; padding: 0; }
.wiki-markdown :deep(blockquote) { border-left: 3px solid var(--cm-border-default); margin: 1em 0; padding: 0.5em 1em; color: var(--cm-text-secondary); }
.wiki-markdown :deep(table) { border-collapse: collapse; width: 100%; margin: 1em 0; }
.wiki-markdown :deep(th), .wiki-markdown :deep(td) { border: 1px solid var(--cm-border-default); padding: 0.5em 0.75em; text-align: left; }
.wiki-markdown :deep(th) { background: var(--cm-bg-surface-muted); font-weight: 600; }
.wiki-markdown :deep(img) { max-width: 100%; border-radius: 8px; }
</style>
