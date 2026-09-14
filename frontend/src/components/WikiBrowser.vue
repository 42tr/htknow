<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { marked } from 'marked'
import DOMPurify from 'dompurify'
import { api } from '../api.js'
import { vDialog } from '../dialog'

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
let pollTimer = null
let requestSeq = 0

const groups = computed(() => indexData.value?.groups || [])
const intro = computed(() => indexData.value?.intro || '')
const pendingTasks = computed(() => status.value?.pending_tasks || 0)
const building = computed(() => pendingTasks.value > 0 || (status.value?.builds?.running || 0) > 0)
const enabled = computed(() => Boolean(status.value?.enabled))
const hasPages = computed(() => groups.value.some((group) => group.items?.length))
const activeSearch = computed(() => searchQuery.value.trim().length > 0)

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
            <p v-if="loadingPage" class="wiki-empty-line">正在加载页面…</p>
            <template v-else-if="detail">
              <header class="wiki-page-head">
                <div>
                  <h3>{{ detail.page.title }}</h3>
                  <p class="wiki-page-meta">
                    <span>{{ detail.page.page_type }}</span>
                    <span>v{{ detail.page.version }}</span>
                    <span>更新于 {{ new Date(detail.page.updated_at * 1000).toLocaleString('zh-CN') }}</span>
                    <span v-if="detail.page.aliases?.length">别名：{{ detail.page.aliases.join('、') }}</span>
                  </p>
                </div>
                <p v-if="detail.page.summary" class="wiki-page-summary">{{ detail.page.summary }}</p>
              </header>

              <article class="wiki-markdown" v-html="bodyHtml"></article>

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
          </div>
        </div>
      </section>
    </div>
  </Teleport>
</template>
