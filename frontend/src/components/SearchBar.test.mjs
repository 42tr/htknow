import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import assert from 'node:assert/strict'
import test from 'node:test'

function subject() {
  const source = readFileSync(new URL('./SearchBar.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import .*$/gm, '')
  const calls = []
  const events = []
  const wiki = { wiki: { page: { id: 1, slug: 'concept/test' } } }
  const sandbox = {
    ref: (value) => ({ value }),
    computed: (value) => typeof value === 'function'
      ? { get value() { return value() } }
      : { get value() { return value.get() }, set value(v) { value.set(v) } },
    watch: (state, callback) => callback(state.value),
    currentKb: { value: { id: 7 } },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    defineEmits: () => (...args) => events.push(args),
    api: {
      search: async (...args) => { calls.push(['search', ...args]); return [wiki] },
      searchImage: async (...args) => { calls.push(['image', ...args]); return [] },
    },
  }
  vm.runInNewContext(`${source}\nglobalThis.subject = { query, searchMode, retrievalMode, handleSearch, imageFile };`, sandbox)
  return { ...sandbox.subject, calls, events, wiki }
}

test('default search uses mixed retrieval and forwards Wiki results', async () => {
  const s = subject()
  s.query.value = '主要内容'
  await s.handleSearch()
  assert.equal(s.calls[0][0], 'search')
  assert.equal(s.calls[0][2], 7)
  assert.equal(s.events.find(([event]) => event === 'search')[1][0], s.wiki)
})

test('returning from advanced mode uses mixed retrieval', async () => {
  const s = subject()
  s.query.value = 'question'
  s.searchMode.value = 'advanced'
  await s.handleSearch()
  assert.equal(s.events[0][0], 'advanced-search')
  assert.equal(s.calls.length, 0)
  s.retrievalMode.value = 'normal'
  await s.handleSearch()
  assert.equal(s.calls[0][0], 'search')
})

test('image search keeps its own request path', async () => {
  const s = subject()
  s.searchMode.value = 'image'
  s.imageFile.value = { name: 'test.png' }
  await s.handleSearch()
  assert.equal(s.calls[0][0], 'image')
  assert.equal(s.calls[0][3], 7)
})
