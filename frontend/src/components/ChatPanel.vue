<script setup>
import { ref, nextTick, onBeforeUnmount } from 'vue'
import { Marked } from 'marked'
import DOMPurify from 'dompurify'
import { api } from '../api'
import { conversationHistory, citationExtension } from '../chatStream.js'
import FileDetail from './FileDetail.vue'
import WikiBrowser from './WikiBrowser.vue'

const emit = defineEmits(['busy'])
const turns = ref([])
const busy = ref(false)
const stage = ref('')
const scrollBox = ref(null)
const followAnswer = () => {
  const box = scrollBox.value
  if (!box || box.scrollHeight - box.scrollTop - box.clientHeight > 100) return
  nextTick(() => { if (box.offsetParent) box.scrollTop = box.scrollHeight })
}
const selectedWiki = ref(null)
const selectedFile = ref(null)
const selectedSlice = ref(null)
const sourceError = ref('')
const notice = ref('')
let controller = null
let generation = 0
let sourceRequest = 0
const setBusy = (value) => { busy.value = value; emit('busy', value) }
const stop = () => controller?.abort()
const clear = () => {
  generation++
  controller?.abort()
  controller = null
  setBusy(false)
  turns.value = []
  selectedWiki.value = null
  selectedFile.value = null
  selectedSlice.value = null
  sourceError.value = ''
  stage.value = ''
  sourceRequest++
  notice.value = ''
}
const render = (turn) => {
  const markdown = new Marked({ breaks: true, extensions: [citationExtension(turn.sources.map((s) => s.id))] })
  // 禁用用户/模型提供的原始 HTML，只有渲染器生成的引用按钮可以带 data-citation。
  markdown.use({ renderer: { html: () => '' } })
  return DOMPurify.sanitize(markdown.parse(turn.answer), { ADD_ATTR: ['data-citation'], FORBID_TAGS: ['img'] })
}
const ask = async ({ question, kbId }) => {
  if (busy.value || !question.trim()) return
  const scope = kbId ?? null
  if (turns.value.length && turns.value.at(-1).scope !== scope) {
    clear()
    notice.value = '已切换知识库，开始新的对话。'
  }
  const messages = conversationHistory(turns.value, scope)
  const turn = { question: question.trim(), answer: '', sources: [], scope, complete: false, error: '', stopped: false, truncated: false }
  turns.value.push(turn)
  nextTick(() => { if (scrollBox.value) scrollBox.value.scrollTop = scrollBox.value.scrollHeight })
  const index = turns.value.length - 1
  const run = ++generation
  controller = new AbortController()
  setBusy(true)
  stage.value = '正在思考…'
  try {
    await api.chat({ question: turn.question, kb_id: scope, messages }, {
      signal: controller.signal,
      onEvent(event, data) {
        if (run !== generation) return
        const current = turns.value[index]
        if (event === 'status') stage.value = ({ searching: '正在检索知识…', thinking: '正在思考…', generating: '正在生成回答…' })[data.stage] || '正在处理…'
        if (event === 'sources') current.sources = data.sources || []
        if (event === 'delta') { stage.value = '正在生成回答…'; followAnswer(); current.answer += data.text || '' }
        if (event === 'done') {
          current.truncated = data.finish_reason === 'length'
          current.complete = !current.truncated && ['stop', 'no_sources'].includes(data.finish_reason)
          if (!current.complete && !current.truncated) current.error = '模型未正常完成回答，请重试。'
        }
      },
    })
  } catch (error) {
    if (run !== generation) return
    if (error.name === 'AbortError') turns.value[index].stopped = true
    else turns.value[index].error = error.message || '对话失败，请重试'
  } finally {
    if (run === generation) { controller = null; setBusy(false) }
  }
}
const openSource = async (source) => {
  sourceError.value = ''
  selectedFile.value = null
  const request = ++sourceRequest
  if (source.result.wiki) { selectedWiki.value = source.result; return }
  selectedWiki.value = null
  try {
    const file = await api.getFile(source.result.file_id)
    if (request !== sourceRequest) return
    selectedFile.value = file
    selectedSlice.value = source.result.id
  } catch (error) { if (request === sourceRequest) sourceError.value = error.message }
}
const openWikiFile = async ({ id }) => {
  const request = ++sourceRequest
  sourceError.value = ''
  try {
    const file = await api.getFile(id)
    if (request !== sourceRequest) return
    selectedFile.value = file
    selectedSlice.value = null
  } catch (error) { if (request === sourceRequest) sourceError.value = error.message }
}
const handleAnswerClick = (event, turn) => {
  const citation = event.target.closest('button[data-citation]')
  if (citation) {
    const source = turn.sources.find((source) => String(source.id) === citation.dataset.citation)
    if (source) openSource(source)
  }
}
const retry = async (turn) => {
  if (busy.value || turns.value.at(-1) !== turn) return
  const question = turn.question
  const kbId = turn.scope
  turns.value.pop()
  await ask({ question, kbId })
}
onBeforeUnmount(() => { generation++; sourceRequest++; controller?.abort() })
defineExpose({ ask })
</script>

<template>
  <section class="chat-panel mx-auto max-w-4xl" aria-label="知识库对话">
    <div v-if="turns.length" class="flex justify-between items-center py-4">
      <span class="text-xs text-slate-400">对话仅保留在当前页面，刷新后清空</span>
      <button class="plain-button" @click="clear">新对话</button>
    </div>
    <p v-if="notice" class="text-sm text-slate-500">{{ notice }}</p>
    <div ref="scrollBox" class="chat-messages">
    <article v-for="(turn, index) in turns" :key="index" class="chat-turn">
      <div class="chat-question">{{ turn.question }}</div>
      <div class="chat-answer" :aria-busy="busy && index === turns.length - 1" @click="handleAnswerClick($event, turn)" v-html="render(turn)"></div>
      <p v-if="busy && index === turns.length - 1" role="status" class="text-sm text-slate-500 py-2">{{ stage }}</p>
      <p v-if="turn.error" role="alert" class="text-red-600 text-sm py-2">{{ turn.error }}</p>
      <p v-if="turn.stopped" class="text-slate-500 text-sm py-2">已停止生成，以上回答可能不完整。</p>
      <p v-if="turn.truncated" class="text-amber-700 text-sm py-2">回答达到长度上限，请缩小问题范围后重试。</p>
      <details v-if="turn.sources.length" class="chat-sources">
        <summary>本轮参考资料 · {{ turn.sources.length }} 条（点击回答中的编号可查看出处）</summary>
        <button v-for="source in turn.sources" :key="source.id" class="chat-source" @click="openSource(source)">
          <span>[{{ source.id }}] {{ source.result.wiki ? 'Wiki · ' : '原文 · ' }}{{ source.result.wiki?.page.title || source.result.file?.filename || '资料' }}</span>
          <small>{{ source.result.content.slice(0, 180) }}</small>
        </button>
      </details>
      <button v-if="!busy && index === turns.length - 1" class="secondary-button mt-3" @click="retry(turn)">重新生成</button>
    </article>
    </div>
    <button v-if="busy" class="secondary-button mt-3" @click="stop">停止生成</button>
    <p v-if="sourceError" role="alert" class="text-red-600 text-sm">{{ sourceError }}</p>
    <WikiBrowser v-if="selectedWiki" :kb-id="selectedWiki.wiki.page.kb_id" :initial-slug="selectedWiki.wiki.page.slug" @locate-file="openWikiFile" @close="selectedWiki = null" />
    <FileDetail v-if="selectedFile" :file="selectedFile" :slice-id="selectedSlice" @close="selectedFile = null" />
  </section>
</template>

<style scoped>
.chat-messages { max-height: 65vh; overflow-y: auto; scrollbar-gutter: stable; padding-right: .5rem; }
.chat-turn { padding: 1.25rem 0; border-bottom: 1px solid #e2e8f0; }
.chat-question { margin: 0 0 1.25rem auto; padding: .8rem 1rem; border-radius: 1rem; background: #f1f5f9; max-width: 85%; width: fit-content; white-space: pre-wrap; overflow-wrap: anywhere; }
.chat-answer { line-height: 1.85; color: #334155; overflow-wrap: anywhere; }
.chat-answer :deep(p) { margin: .6rem 0; }
.chat-answer :deep(h1), .chat-answer :deep(h2), .chat-answer :deep(h3) { font-weight: 650; margin: 1rem 0 .5rem; }
.chat-answer :deep(ul), .chat-answer :deep(ol) { padding-left: 1.5rem; list-style: revert; }
.chat-answer :deep(pre) { overflow: auto; background: #f1f5f9; padding: 1rem; border-radius: .5rem; }
.chat-answer :deep(table) { display: block; overflow-x: auto; border-collapse: collapse; }
.chat-answer :deep(td), .chat-answer :deep(th) { border: 1px solid #e2e8f0; padding: .4rem .7rem; }
.chat-answer :deep(.chat-citation) { color: #2563eb; background: #eff6ff; padding: 0 .25rem; border-radius: .25rem; cursor: pointer; font-size: .8rem; vertical-align: super; }
.chat-sources { margin-top: 1rem; font-size: .8rem; color: #64748b; }
.chat-sources summary { cursor: pointer; }
.chat-source { display: block; width: 100%; text-align: left; padding: .7rem; margin-top: .5rem; background: #f8fafc; border: 1px solid #e2e8f0; border-radius: .5rem; }
.chat-source span, .chat-source small { display: block; }
.chat-source span { color: #334155; font-weight: 600; }
.chat-source small { margin-top: .3rem; }
</style>
