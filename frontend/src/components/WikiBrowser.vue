<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { marked } from 'marked'
import DOMPurify from 'dompurify'
import { api } from '../api.js'
import { vDialog } from '../dialog'
import WikiRevisionDrawer from './WikiRevisionDrawer.vue'

const props = defineProps({
  kbId: { type: [Number, String], required: true },
  title: { type: String, default: '知识库 Wiki' },
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
// 手工条目只能是这两类：摘要页绑定文档、索引页是系统目录。
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
// P2：人工编辑、历史版本、归档、体检、新建条目
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
// 索引页由系统维护，不允许人工编辑；没有编辑权限时只读。
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

// `[[slug|名称]]` 是管道生成的内部链接。转成 `#wiki/<slug>` 片段后再交给 marked，
// 这样正文里的代码块/已有链接不会被误伤（跳过的逻辑与后端 linkify 一致）。
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
    indexData.value = await api.getWikiIndex(props.kbId)
    for (const group of groups.value) expanded.value[group.page_type] = true
  } catch (e) {
    error.value = e?.message || '加载 Wiki 目录失败'
  } finally {
    loadingIndex.value = false
  }
}

const loadStatus = async () => {
  try {
    status.value = await api.getWikiStatus(props.kbId)
  } catch (e) {
    if (!error.value) error.value = e?.message || '加载 Wiki 状态失败'
  }
}

const loadConfig = async () => {
  try {
    config.value = await api.getWikiConfig(props.kbId)
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
    const data = await api.getWikiPage(props.kbId, target)
    if (request !== requestSeq) return
    detail.value = data
    currentSlug.value = data.page.slug
    searchQuery.value = ''
    searchResults.value = null
    // 换页时退出编辑态，避免把上一页的草稿保存到这一页。
    editing.value = false
    document.querySelector('.wiki-page-body')?.scrollTo({ top: 0 })
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
    const data = await api.searchWikiPages(props.kbId, query, 30)
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

const openSlice = (slice, source) => {
  const params = new URLSearchParams({ file_id: String(slice.file_id), slice_id: String(slice.slice_id) })
  const viewer = /\.(xlsx|xls)$/i.test(source?.filename || '') ? 'excel-viewer.html' : 'pdf-highlight.html'
  window.open(`/${viewer}?${params.toString()}`, '_blank', 'noopener')
}

const slicesOf = (fileId) => (detail.value?.slices || []).filter((slice) => slice.file_id === fileId)

const locateFile = (source) => {
  emit('locate-file', { id: source.file_id, kb_id: props.kbId })
  emit('close')
}

const saveConfig = async () => {
  savingConfig.value = true
  error.value = ''
  try {
    config.value = await api.updateWikiConfig({
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
    const result = await api.rebuildWiki(props.kbId)
    flash(`已入队 ${result.enqueued} 个文件，生成中…`)
    await Promise.all([loadStatus(), loadIndex()])
  } catch (e) {
    error.value = e?.message || '触发 Wiki 重建失败'
  } finally {
    rebuilding.value = false
  }
}

const issueLabel = (kind) => ISSUE_LABELS[kind] || kind

const parseAliases = (value) =>
  (value || '')
    .split(/[,，、;；\n]/)
    .map((item) => item.trim())
    .filter(Boolean)

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
    detail.value = await api.updateWikiPage({
      kb_id: props.kbId,
      slug: currentSlug.value,
      title: editForm.value.title.trim(),
      summary: editForm.value.summary,
      content: editForm.value.content,
      aliases: parseAliases(editForm.value.aliases),
    })
    editing.value = false
    flash(`已保存为 v${detail.value.page.version}`)
    // 标题变化会影响目录与交叉链接，重新拉一次索引。
    await loadIndex()
  } catch (e) {
    error.value = e?.message || '保存 Wiki 页面失败'
  } finally {
    savingEdit.value = false
  }
}

const changeStatus = async (status) => {
  error.value = ''
  try {
    detail.value = await api.updateWikiPage({ kb_id: props.kbId, slug: currentSlug.value, status })
    flash(status === 'archived' ? '已归档：退出目录与搜索，可在「已归档」中恢复' : '已恢复发布')
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
    await api.deleteWikiPage(props.kbId, currentSlug.value)
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
    const data = await api.listWikiPages(props.kbId, { status: 'archived', limit: 200 })
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
    lint.value = await api.lintWiki(props.kbId)
  } catch (e) {
    lint.value = null
    error.value = e?.message || 'Wiki 体检失败'
  } finally {
    lintLoading.value = false
  }
}

const toggleLint = async () => {
  showLint.value = !showLint
  showCreate.value = false
  if (showLint.value) await loadLint()
}

const openIssuePage = async (issue) => {
  showLint.value = false
  await openPage(issue.slug)
}

// 立即跑一次后端收敛：清死链、补交叉链接、重算入链与索引目录。
const rebuildLinks = async () => {
  relinking.value = true
  error.value = ''
  try {
    const report = await api.rebuildWikiLinks(props.kbId)
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
    const created = await api.createWikiPage({
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
    <div class="upload-overlay" @click.self="emit('close')" @keydown.esc="emit('close')">
      <section v-dialog class="wiki-dialog" role="dialog" aria-modal="true" :aria-label="title">
        <div class="panel-heading">
          <div>
            <h2>{{ title }}</h2>
            <p>{{ subtitle || '由文档自动生成的知识库条目与交叉链接。' }}</p>
          </div>
          <div class="wiki-heading-actions">
            <span v-if="statusText" class="wiki-badge" :class="{ busy: building }">{{ statusText }}</span>
            <button v-if="canEdit" class="plain-button" :aria-expanded="showCreate" @click="showCreate = !showCreate; showLint = false">
              {{ showCreate ? '收起新建' : '新建条目' }}
            </button>
            <button class="plain-button" :aria-expanded="showLint" @click="toggleLint">
              {{ showLint ? '收起体检' : '体检' }}
            </button>
            <button class="plain-button" :aria-expanded="showSettings" @click="showSettings = !showSettings">
              {{ showSettings ? '收起设置' : '设置' }}
            </button>
            <button class="plain-button" aria-label="关闭 Wiki" @click="emit('close')">✕</button>
          </div>
        </div>

        <div v-if="showSettings" class="wiki-settings">
          <label class="wiki-setting">
            <input v-model="draft.enabled" type="checkbox" :disabled="!canEdit" />
            <span>为本知识库生成 Wiki</span>
          </label>
          <label class="wiki-setting">
            <span>粒度</span>
            <select v-model="draft.granularity" :disabled="!canEdit">
              <option v-for="item in GRANULARITIES" :key="item.value" :value="item.value">{{ item.label }}</option>
            </select>
          </label>
          <label class="wiki-setting">
            <span>单文件条目上限</span>
            <input v-model.number="draft.max_pages_per_ingest" type="number" min="0" max="200" :disabled="!canEdit" />
          </label>
          <div class="wiki-setting-actions">
            <span v-if="config && !config.llm_available" class="wiki-warn">未配置 LLM，无法生成内容</span>
            <span v-if="!canEdit" class="wiki-warn">需要编辑权限才能修改</span>
            <button class="secondary-button" :disabled="!canEdit || savingConfig" @click="saveConfig">
              {{ savingConfig ? '保存中…' : '保存' }}
            </button>
            <button class="secondary-button" :disabled="!canEdit || rebuilding || !enabled" @click="rebuild">
              {{ rebuilding ? '入队中…' : '重建全库' }}
            </button>
          </div>
        </div>

        <p v-if="error" class="inline-error wiki-error">{{ error }}</p>
        <p v-if="notice" class="wiki-notice">{{ notice }}</p>

        <div class="wiki-body">
          <aside class="wiki-sidebar">
            <div class="wiki-search">
              <input
                v-model="searchQuery"
                type="search"
                placeholder="搜索条目…"
                aria-label="搜索 Wiki 条目"
                @keyup.enter="runSearch"
              />
              <button class="secondary-button" :disabled="searching" @click="runSearch">搜索</button>
            </div>
            <div class="wiki-sidebar-tools">
              <button class="plain-button" :class="{ active: archivedOnly }" @click="toggleArchived">
                {{ archivedOnly ? '返回目录' : '已归档' }}
              </button>
            </div>

            <ul v-if="activeSearch && searchResults" class="wiki-tree">
              <li v-for="item in searchResults" :key="item.slug">
                <button class="wiki-tree-item" :class="{ active: item.slug === currentSlug }" @click="openPage(item.slug)">
                  <span class="wiki-tree-title">{{ item.title }}</span>
                  <span class="wiki-tree-type">{{ item.page_type }}</span>
                </button>
                <p v-if="item.summary" class="wiki-tree-summary">{{ item.summary }}</p>
              </li>
              <li v-if="!searchResults.length" class="wiki-empty-line">没有匹配的条目</li>
            </ul>

            <ul v-else-if="archivedOnly" class="wiki-tree">
              <li v-for="item in archivedPages" :key="item.slug">
                <button class="wiki-tree-item" :class="{ active: item.slug === currentSlug }" @click="openPage(item.slug)">
                  <span class="wiki-tree-title">{{ item.title }}</span>
                  <span class="wiki-tree-type">已归档</span>
                </button>
              </li>
              <li v-if="loadingArchived" class="wiki-empty-line">正在加载归档页面…</li>
              <li v-else-if="!archivedPages.length" class="wiki-empty-line">没有归档页面。</li>
            </ul>

            <template v-else>
              <button class="wiki-tree-item wiki-index-entry" :class="{ active: currentSlug === INDEX_SLUG }" @click="openPage(INDEX_SLUG)">
                <span class="wiki-tree-title">总览</span>
              </button>
              <div v-for="group in groups" :key="group.page_type" class="wiki-group">
                <button class="wiki-group-head" :aria-expanded="!!expanded[group.page_type]" @click="expanded[group.page_type] = !expanded[group.page_type]">
                  <span>{{ group.title }}</span><b>{{ group.count }}</b>
                </button>
                <ul v-if="expanded[group.page_type]" class="wiki-tree">
                  <li v-for="item in group.items" :key="item.slug">
                    <button class="wiki-tree-item" :class="{ active: item.slug === currentSlug }" :title="item.summary" @click="openPage(item.slug)">
                      <span class="wiki-tree-title">{{ item.title }}</span>
                    </button>
                  </li>
                </ul>
              </div>
              <p v-if="loadingIndex" class="wiki-empty-line">正在加载目录…</p>
              <p v-else-if="!hasPages" class="wiki-empty-line">
                还没有 Wiki 条目。在「设置」中开启后，重新解析或点击「重建全库」即可生成。
              </p>
            </template>
          </aside>

          <div class="wiki-page-body" @click="onBodyClick">
            <section v-if="showLint" class="wiki-panel">
              <div class="wiki-panel-head">
                <h3>体检结果</h3>
                <div class="wiki-panel-actions">
                  <button class="secondary-button" :disabled="lintLoading" @click="loadLint">
                    {{ lintLoading ? '检查中…' : '重新检查' }}
                  </button>
                  <button class="secondary-button" :disabled="relinking || !canEdit" @click="rebuildLinks">
                    {{ relinking ? '收敛中…' : '重建交叉链接' }}
                  </button>
                </div>
              </div>
              <p v-if="lint" class="wiki-lint-summary">
                扫描 {{ lint.scanned }} 个页面，发现 {{ lint.issue_count }} 个问题
                <template v-if="lint.truncated">（仅列出前 {{ lintIssues.length }} 条）</template>
                <span v-for="kind in lintKinds" :key="kind.kind" class="wiki-chip">
                  {{ issueLabel(kind.kind) }} {{ kind.count }}
                </span>
              </p>
              <ul v-if="lintIssues.length" class="wiki-lint-list">
                <li v-for="(issue, index) in lintIssues" :key="`${issue.kind}-${issue.page_id}-${index}`" :class="issue.severity">
                  <button class="wiki-lint-page" @click="openIssuePage(issue)">{{ issue.title || issue.slug }}</button>
                  <span class="wiki-lint-kind">{{ issueLabel(issue.kind) }}</span>
                  <span class="wiki-lint-message">{{ issue.message }}</span>
                </li>
              </ul>
              <p v-else-if="lint && !lintLoading" class="wiki-empty-line">没有发现问题。</p>
              <p class="wiki-lint-hint">
                体检只报告问题、不自动改写内容；「重建交叉链接」是唯一可确定性修复的动作，其余请在页面上手工编辑。
              </p>
            </section>

            <section v-else-if="showCreate" class="wiki-panel">
              <div class="wiki-panel-head"><h3>新建条目</h3></div>
              <form class="wiki-edit" @submit.prevent="submitCreate">
                <label class="wiki-field">
                  <span>标题</span>
                  <input v-model="createForm.title" type="text" maxlength="200" required />
                </label>
                <label class="wiki-field">
                  <span>类型</span>
                  <select v-model="createForm.page_type">
                    <option v-for="item in CREATABLE_TYPES" :key="item.value" :value="item.value">{{ item.label }}</option>
                  </select>
                </label>
                <label class="wiki-field">
                  <span>摘要</span>
                  <input v-model="createForm.summary" type="text" maxlength="2000" placeholder="一句话说明，出现在目录与搜索结果里" />
                </label>
                <label class="wiki-field">
                  <span>正文（Markdown，内链写作 [[slug|显示名]]）</span>
                  <textarea v-model="createForm.content" rows="12"></textarea>
                </label>
                <div class="wiki-edit-actions">
                  <span class="wiki-warn">slug 由标题自动生成；手工条目不会被自动生成覆盖。</span>
                  <button type="button" class="secondary-button" @click="showCreate = false">取消</button>
                  <button type="submit" class="secondary-button" :disabled="creating">{{ creating ? '创建中…' : '创建' }}</button>
                </div>
              </form>
            </section>

            <template v-else>
            <p v-if="loadingPage" class="wiki-empty-line">正在加载页面…</p>
            <template v-else-if="detail">
              <header class="wiki-page-head">
                <div>
                  <h3>{{ detail.page.title }}</h3>
                  <p class="wiki-page-meta">
                    <span>{{ detail.page.page_type }}</span>
                    <span>v{{ detail.page.version }}</span>
                    <span>更新于 {{ new Date(detail.page.updated_at * 1000).toLocaleString('zh-CN') }}</span>
                    <span v-if="editSourceLabel">{{ editSourceLabel }}</span>
                    <span v-if="isArchived" class="wiki-badge">已归档</span>
                    <span v-if="detail.page.aliases?.length">别名：{{ detail.page.aliases.join('、') }}</span>
                  </p>
                </div>
                <p v-if="detail.page.summary" class="wiki-page-summary">{{ detail.page.summary }}</p>
              </header>

              <div class="wiki-page-actions">
                <button class="plain-button" @click="showRevisions = true">历史版本</button>
                <template v-if="canEditPage">
                  <button v-if="!editing" class="plain-button" @click="startEdit">编辑</button>
                  <button v-if="isArchived" class="plain-button" @click="changeStatus('published')">恢复发布</button>
                  <button v-else class="plain-button" @click="changeStatus('archived')">归档</button>
                  <button class="plain-button danger" @click="removePage">删除</button>
                </template>
                <span v-else-if="isIndexPage" class="wiki-lint-hint">索引页由系统维护，不可手工编辑。</span>
              </div>

              <form v-if="editing" class="wiki-edit" @submit.prevent="saveEdit">
                <label class="wiki-field">
                  <span>标题</span>
                  <input v-model="editForm.title" type="text" maxlength="200" required />
                </label>
                <label class="wiki-field">
                  <span>摘要</span>
                  <input v-model="editForm.summary" type="text" maxlength="2000" placeholder="一句话说明，出现在目录与搜索结果里" />
                </label>
                <label class="wiki-field">
                  <span>别名</span>
                  <input v-model="editForm.aliases" type="text" placeholder="用顿号或逗号分隔，用于交叉链接匹配" />
                </label>
                <label class="wiki-field">
                  <span>正文（Markdown，内链写作 [[slug|显示名]]）</span>
                  <textarea v-model="editForm.content" rows="18"></textarea>
                </label>
                <div class="wiki-edit-actions">
                  <span class="wiki-warn">保存前会自动留下版本快照，可随时回滚；自动生成的内容不会再覆盖人工编辑。</span>
                  <button type="button" class="secondary-button" :disabled="savingEdit" @click="editing = false">取消</button>
                  <button type="submit" class="secondary-button" :disabled="savingEdit">
                    {{ savingEdit ? '保存中…' : '保存' }}
                  </button>
                </div>
              </form>

              <article v-else class="wiki-markdown" v-html="bodyHtml"></article>

              <section v-if="related.outgoing.length || related.incoming.length" class="wiki-links">
                <div v-if="related.outgoing.length">
                  <h4>链接到</h4>
                  <div class="wiki-chips">
                    <button v-for="link in related.outgoing" :key="`out-${link.slug}`" @click="openPage(link.slug)">
                      {{ link.title }}
                    </button>
                  </div>
                </div>
                <div v-if="related.incoming.length">
                  <h4>被引用</h4>
                  <div class="wiki-chips">
                    <button v-for="link in related.incoming" :key="`in-${link.slug}`" @click="openPage(link.slug)">
                      {{ link.title }}
                    </button>
                  </div>
                </div>
              </section>

              <section v-if="detail.sources?.length" class="wiki-sources">
                <h4>来源（{{ detail.sources.length }} 个文件 · {{ detail.slices?.length || 0 }} 处证据）</h4>
                <div v-for="source in detail.sources" :key="source.file_id" class="wiki-source">
                  <button class="wiki-source-name" :title="source.filename" @click="locateFile(source)">
                    {{ source.filename }}
                  </button>
                  <div v-if="slicesOf(source.file_id).length" class="wiki-chips">
                    <button v-for="slice in slicesOf(source.file_id)" :key="slice.slice_id" @click="openSlice(slice, source)">
                      原文 #{{ slice.slice_id }}
                    </button>
                  </div>
                </div>
              </section>
            </template>
            <div v-else-if="!error" class="wiki-empty">
              <h3>还没有内容</h3>
              <p>Wiki 会在文档解析完成后自动生成；也可以在「设置」里手动触发重建。</p>
            </div>
            </template>
          </div>
        </div>

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
