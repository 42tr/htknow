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
  const revoked = []
  let unmount
  const sandbox = {
    URL: {
      createObjectURL: (file) => `blob:${file.name}`,
      revokeObjectURL: (url) => revoked.push(url),
    },
    onBeforeUnmount: (callback) => { unmount = callback },
    ref: (value) => ({ value }),
    computed: (value) => typeof value === 'function'
      ? { get value() { return value() } }
      : { get value() { return value.get() }, set value(v) { value.set(v) } },
    watch: (state, callback) => callback(state.value),
    currentKb: { value: { id: 7 } },
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} },
    defineProps: () => ({ chatBusy: false }),
    defineEmits: () => (...args) => events.push(args),
    api: {
      search: async (...args) => { calls.push(['search', ...args]); return [wiki] },
      searchImage: async (...args) => { calls.push(['image', ...args]); return [] },
    },
  }
  vm.runInNewContext(`${source}\nglobalThis.subject = { query, handleSearch, handleSubmit, handleKeydown, imageFile, handlePaste, clearImage, imagePreview, canSubmit };`, sandbox)
  return { ...sandbox.subject, calls, events, wiki, revoked, unmount }
}

test('search-only uses mixed retrieval and forwards Wiki results', async () => {
  const s = subject()
  s.query.value = '主要内容'
  await s.handleSearch()
  assert.equal(s.calls[0][0], 'search')
  assert.equal(s.calls[0][2], 7)
  assert.equal(s.events.find(([event]) => event === 'search')[1][0], s.wiki)
})

function paste(s, file) {
  let prevented = false
  s.handlePaste({
    clipboardData: { items: [{ type: file.type, getAsFile: () => file }] },
    preventDefault: () => { prevented = true },
  })
  return prevented
}

test('pasted image selects image search without requiring text', async () => {
  const s = subject()
  const file = { name: 'test.png', type: 'image/png' }
  assert.equal(paste(s, file), true)
  assert.equal(s.canSubmit.value, true)
  assert.equal(s.imagePreview.value, 'blob:test.png')
  await s.handleSearch()
  assert.equal(s.calls[0][0], 'image')
  assert.equal(s.calls[0][1], file)
  assert.equal(s.calls[0][2], '')
  assert.equal(s.calls[0][3], 7)
})

test('image search includes text and removal restores text search', async () => {
  const s = subject()
  s.query.value = '发动机'
  paste(s, { name: 'test.png', type: 'image/png' })
  await s.handleSearch()
  assert.equal(s.calls[0][0], 'image')
  assert.equal(s.calls[0][2], '发动机')
  s.clearImage()
  await s.handleSearch()
  assert.equal(s.calls[1][0], 'search')
  assert.equal(s.calls[1][1], '发动机')
  assert.deepEqual(s.revoked, ['blob:test.png'])
})

test('text paste keeps browser behavior and does not activate image search', () => {
  const s = subject()
  assert.equal(paste(s, { type: 'text/plain' }), false)
  assert.equal(s.imageFile.value, null)
  s.handlePaste({})
  assert.equal(s.canSubmit.value, false)
})

test('replacing an image and unmounting release preview URLs', () => {
  const s = subject()
  paste(s, { name: 'first.png', type: 'image/png' })
  paste(s, { name: 'second.png', type: 'image/png' })
  assert.equal(s.imageFile.value.name, 'second.png')
  assert.deepEqual(s.revoked, ['blob:first.png'])
  s.unmount()
  assert.deepEqual(s.revoked, ['blob:first.png', 'blob:second.png'])
})

test('empty input does not submit after removing the image', async () => {
  const s = subject()
  paste(s, { name: 'test.png', type: 'image/png' })
  s.clearImage()
  assert.equal(s.canSubmit.value, false)
  await s.handleSearch()
  assert.equal(s.calls.length, 0)
})


test('default submit starts a conversation with selected scope instead of searching', () => {
  const s = subject()
  s.query.value = '螺旋桨的主要类型'
  s.handleSubmit()
  assert.equal(s.calls.length, 0)
  assert.equal(s.events[0][0], 'chat')
  assert.equal(s.events[0][1].question, '螺旋桨的主要类型')
  assert.equal(s.events[0][1].kbId, 7)
  assert.equal(s.query.value, '')
})

test('enter starts chat but composing input does not submit', () => {
  const s = subject()
  s.query.value = '问题'
  s.handleKeydown({ key: 'Enter', isComposing: true })
  assert.equal(s.events.length, 0)
  s.handleKeydown({ key: 'Enter', isComposing: false })
  assert.equal(s.events[0][0], 'chat')
})


test('Shift+Enter and IME confirmation preserve the draft; Enter prevents a newline', () => {
  const s = subject()
  s.query.value = '多行\n问题'
  s.handleKeydown({ key: 'Enter', shiftKey: true })
  s.handleKeydown({ key: 'Enter', keyCode: 229 })
  assert.equal(s.events.length, 0)
  let prevented = false
  s.handleKeydown({ key: 'Enter', preventDefault: () => { prevented = true } })
  assert.equal(prevented, true)
  assert.equal(s.events[0][1].question, '多行\n问题')
})

test('overlong questions preserve the draft and Unicode uses the server character limit', () => {
  const s = subject()
  s.query.value = '字'.repeat(4001)
  s.handleSubmit()
  assert.equal(s.events.length, 0)
  assert.equal(s.query.value.length, 4001)
  s.query.value = '😀'.repeat(4000)
  s.handleSubmit()
  assert.equal(s.events.length, 1)
  assert.equal(s.query.value, '')
})
