import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import assert from 'node:assert/strict'
import test from 'node:test'
import { conversationHistory } from '../chatStream.js'

function subject(chat) {
  const source = readFileSync(new URL('./ChatPanel.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import .*$/gm, '')
  const requests = []
  const events = []
  const sandbox = {
    AbortController, conversationHistory,
    ref: (value) => ({ value }), nextTick: async (fn) => fn?.(),
    onBeforeUnmount() {}, defineExpose() {}, defineEmits: () => (...args) => events.push(args),
    api: {
      chat: async (request, options) => {
        requests.push(request)
        if (chat) return chat(request, options)
        options.onEvent('sources', { sources: [{ id: 1, result: { file_id: 10, id: 20 } }] })
        options.onEvent('delta', { text: '答案[1]' })
        options.onEvent('done', { finish_reason: 'stop' })
      },
      getFile: async (id) => ({ id, filename: '手册.pdf' }),
    },
  }
  vm.runInNewContext(`${source}\nglobalThis.subject = { ask, stop, clear, turns, busy, openSource, selectedFile, selectedSlice };`, sandbox)
  return { ...sandbox.subject, requests, events }
}

test('multi-turn chat sends completed history and resets it when scope changes', async () => {
  const s = subject()
  await s.ask({ question: '类型', kbId: 1 })
  await s.ask({ question: '它的优点', kbId: 1 })
  assert.equal(s.requests[1].messages.length, 2)
  assert.equal(s.requests[1].messages[0].content, '类型')
  assert.equal(s.turns.value[1].complete, true)
  await s.ask({ question: '另一个库', kbId: 2 })
  assert.equal(s.requests[2].messages.length, 0)
  assert.equal(s.turns.value.length, 1)
  s.clear()
  assert.equal(s.turns.value.length, 0)
})

test('citations open the real source file at its slice', async () => {
  const s = subject()
  await s.ask({ question: 'q', kbId: 1 })
  await s.openSource(s.turns.value[0].sources[0])
  assert.equal(s.selectedFile.value.id, 10)
  assert.equal(s.selectedSlice.value, 20)
})

test('stopping preserves partial output but does not mark a turn completed', async () => {
  const s = subject(async (_, { signal, onEvent }) => {
    onEvent('delta', { text: '未完成' })
    await new Promise((resolve, reject) => signal.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError'))))
  })
  const request = s.ask({ question: 'q', kbId: 1 })
  s.stop()
  await request
  assert.equal(s.turns.value[0].answer, '未完成')
  assert.equal(s.turns.value[0].stopped, true)
  assert.equal(s.turns.value[0].complete, false)
  assert.equal(s.busy.value, false)
})

test('clearing a running conversation ignores stale output', async () => {
  let finish
  const s = subject(async (_, { onEvent }) => {
    await new Promise((resolve) => { finish = resolve })
    onEvent('delta', { text: 'stale' })
    onEvent('done', { finish_reason: 'stop' })
  })
  const request = s.ask({ question: 'q', kbId: 1 })
  s.clear()
  finish()
  await request
  assert.equal(s.turns.value.length, 0)
  assert.equal(s.busy.value, false)
})
