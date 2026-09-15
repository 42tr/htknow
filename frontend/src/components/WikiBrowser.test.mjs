import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import assert from 'node:assert/strict'
import test from 'node:test'

// 在没有浏览器的情况下执行组件里真实的 `[[slug|名称]]` 改写逻辑。
function subject() {
  const source = readFileSync(new URL('./WikiBrowser.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1]
    .replace(/^import .*$/gm, '')
  const sandbox = {
    console,
    encodeURIComponent,
    ref: (value) => ({ value }),
    computed: (get) => ({ get value() { return get() } }),
    defineProps: (spec) => Object.fromEntries(Object.entries(spec).map(([key, value]) => [key, value.default])),
    defineEmits: () => () => {},
    watch() {},
    onMounted() {},
    onBeforeUnmount() {},
  }
  vm.runInNewContext(`${source}\nglobalThis.subject = { rewriteWikiLinks };`, sandbox)
  return sandbox.subject
}

const { rewriteWikiLinks } = subject()

test('rewrites labelled wiki links into internal anchors', () => {
  assert.equal(
    rewriteWikiLinks('参见 [[entity/张三|张三]] 与 [[concept/rag|RAG]]。'),
    `参见 [张三](#wiki/${encodeURIComponent('entity/张三')}) 与 [RAG](#wiki/concept%2Frag)。`,
  )
})

test('falls back to the slug when no label is given', () => {
  assert.equal(rewriteWikiLinks('见 [[entity/张三]]'), `见 [entity/张三](#wiki/${encodeURIComponent('entity/张三')})`)
})

test('leaves fenced code blocks alone', () => {
  const source = ['正文 [[concept/rag|RAG]]', '```text', '代码里的 [[concept/rag|RAG]]', '```', '结尾 [[concept/rag|RAG]]'].join('\n')
  const lines = rewriteWikiLinks(source).split('\n')
  assert.match(lines[0], /#wiki\/concept%2Frag/)
  assert.equal(lines[2], '代码里的 [[concept/rag|RAG]]')
  assert.match(lines[4], /#wiki\/concept%2Frag/)
})

test('leaves tilde fences alone too', () => {
  const lines = rewriteWikiLinks('~~~\n[[concept/rag|RAG]]\n~~~').split('\n')
  assert.equal(lines[1], '[[concept/rag|RAG]]')
})

test('drops empty link targets instead of emitting broken anchors', () => {
  assert.equal(rewriteWikiLinks('空的 [[ | 名字]]'), '空的 名字')
})

test('search deep link opens the requested page without falling back to the index', async () => {
  const source = readFileSync(new URL('./WikiBrowser.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import .*$/gm, '')
  const opened = []
  let mounted
  const sandbox = {
    ref: (value) => ({ value }),
    computed: (get) => ({ get value() { return get() } }),
    defineProps: () => ({ kbId: 7, initialSlug: 'concept/orbit' }),
    defineEmits: () => () => {},
    watch() {}, onBeforeUnmount() {}, onMounted: (handler) => { mounted = handler },
    document: { querySelector: () => null },
    api: {
      getWikiIndex: async () => ({ groups: [] }),
      getWikiStatus: async () => ({}),
      getWikiConfig: async () => ({}),
      getWikiPage: async (kbId, slug) => {
        opened.push([kbId, slug])
        return { page: { slug } }
      },
    },
  }
  vm.runInNewContext(source, sandbox)
  await mounted()
  assert.deepEqual(opened, [[7, 'concept/orbit']])
})
