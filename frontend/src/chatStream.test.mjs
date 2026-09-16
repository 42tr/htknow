import test from 'node:test'
import assert from 'node:assert/strict'
import { Marked } from 'marked'
import { consumeChatStream, conversationHistory, citationExtension } from './chatStream.js'

const response = (text) => new Response(new ReadableStream({
  start(controller) {
    for (const byte of new TextEncoder().encode(text)) controller.enqueue(Uint8Array.of(byte))
    controller.close()
  },
}), { headers: { 'content-type': 'text/event-stream' } })

test('streams fragmented Unicode, CRLF, comments and source mapping before answer', async () => {
  const events = []
  await consumeChatStream(response(': alive\r\nevent: sources\r\ndata: {"sources":[{"id":1}]}\r\n\r\nevent: delta\ndata: {"text":"螺旋桨[1]"}\n\nevent: done\ndata: {"finish_reason":"stop"}\n\n'), (type, payload) => events.push([type, payload]))
  assert.deepEqual(events.map(([type]) => type), ['sources', 'delta', 'done'])
  assert.equal(events[1][1].text, '螺旋桨[1]')
})

test('truncated streams, server errors and non-SSE bodies are failures', async () => {
  await assert.rejects(consumeChatStream(response('event: delta\ndata: {"text":"partial"}\n\n'), () => {}), /提前结束/)
  await assert.rejects(consumeChatStream(response('event: error\ndata: {"message":"upstream failed"}\n\n'), () => {}), /upstream failed/)
  await assert.rejects(consumeChatStream(new Response('not SSE'), () => {}), /流式/)
})

test('only completed turns in current knowledge scope become history, with a bounded budget', () => {
  const turn = { scope: 1, complete: true, question: 'q', answer: 'a' }
  assert.deepEqual(conversationHistory([turn, { ...turn, complete: false }, { ...turn, scope: 2 }], 1), [{ role: 'user', content: 'q' }, { role: 'assistant', content: 'a' }])
  assert.equal(conversationHistory(Array(10).fill(turn), 1).length, 12)
  assert.deepEqual(conversationHistory([{ ...turn, answer: 'a'.repeat(16000) }], 1), [])
})

test('citation buttons only resolve current sources and never code blocks or arbitrary HTML', () => {
  const markdown = new Marked({ extensions: [citationExtension([1, 2])] })
  markdown.use({ renderer: { html: () => '' } })
  const html = markdown.parse('支持的事实[1][2]，不存在[99]。\n\n`[1]`\n\n```\n[2]\n```\n\n<button data-citation="99">伪造</button>')
  assert.equal((html.match(/data-citation=/g) || []).length, 2)
  assert.ok(html.includes('<code>[1]</code>'))
  assert.ok(!html.includes('data-citation="99"'))
})
