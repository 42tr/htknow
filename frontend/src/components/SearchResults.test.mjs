import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import assert from 'node:assert/strict'
import test from 'node:test'

function subject(results, getFile = async (id) => ({ id })) {
  const source = readFileSync(new URL('./SearchResults.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import .*$/gm, '')
  let onResultsChanged
  const sandbox = {
    ref: (value) => ({ value }),
    computed: (get) => ({ get value() { return get() } }),
    defineProps: () => ({ results }),
    watch: (_get, handler) => { onResultsChanged = handler },
    api: { getFile },
  }
  vm.runInNewContext(`${source}\nglobalThis.subject = { typeFilter, filteredResults, openWikiSource, sourceFile, chooseResult };`, sandbox)
  return { ...sandbox.subject, reset: () => onResultsChanged() }
}

test('Wiki results have their own filter and retain relevance ordering', () => {
  const wiki = { id: 1, wiki: { page: { id: 1 } }, score: 0.03 }
  const file = { id: 1, file: { id: 4, filename: 'source.pdf' }, score: 0.02 }
  const image = { id: 2, file: { id: 5, filename: 'image.png' }, score: 0.01 }
  const s = subject([image, file, wiki])
  assert.deepEqual(Array.from(s.filteredResults.value), [wiki, file, image])
  for (const [filter, expected] of [['wiki', wiki], ['document', file], ['image', image]]) {
    s.typeFilter.value = filter
    assert.deepEqual(Array.from(s.filteredResults.value), [expected])
  }
})

test('changing search results discards an in-flight Wiki source preview', async () => {
  let resolve
  const s = subject([], () => new Promise((done) => { resolve = done }))
  const pending = s.openWikiSource({ id: 4 })
  s.reset()
  resolve({ id: 4, filename: 'old-source.pdf' })
  await pending
  assert.equal(s.sourceFile.value, null)
})

test('selecting another result cancels an in-flight source preview', async () => {
  let resolve
  const s = subject([], () => new Promise((done) => { resolve = done }))
  const pending = s.openWikiSource({ id: 4 })
  s.chooseResult({ file: { id: 5 } })
  resolve({ id: 4 })
  await pending
  assert.equal(s.sourceFile.value, null)
})
