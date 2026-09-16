// fetch POST + SSE，支持 UTF-8/CRLF 跨网络分片；连接异常结束不能当作成功。
export async function consumeChatStream(response, onEvent) {
  if (!response.body || !response.headers.get('content-type')?.includes('text/event-stream')) {
    throw new Error('服务未返回流式对话数据')
  }
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  let event = 'message'
  let data = []
  let done = false
  const dispatch = () => {
    if (!data.length) { event = 'message'; return }
    const payload = JSON.parse(data.join('\n'))
    if (event === 'error') throw new Error(payload.message || '生成失败，请重试')
    onEvent(event, payload)
    if (event === 'done') done = true
    data = []
    event = 'message'
  }
  const feed = (text) => {
    buffer += text
    if (buffer.length > 4 * 1024 * 1024) throw new Error('对话事件过大')
    let index
    while ((index = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, index).replace(/\r$/, '')
      buffer = buffer.slice(index + 1)
      if (!line) dispatch()
      else if (line.startsWith('event:')) event = line.slice(6).trim()
      else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''))
      if (done) break
    }
  }
  try {
    while (!done) {
      const next = await reader.read()
      feed(decoder.decode(next.value, { stream: !next.done }))
      if (next.done) { feed('\n\n'); break }
    }
    if (!done) throw new Error('连接提前结束，回答可能不完整，请重试')
  } finally {
    await reader.cancel().catch(() => {})
    reader.releaseLock()
  }
}

export function conversationHistory(turns, scope) {
  const pairs = turns.filter((turn) => turn.complete && turn.scope === scope).slice(-6)
  const result = []
  let remaining = 16000
  for (const turn of pairs.reverse()) {
    const length = [...turn.question].length + [...turn.answer].length
    if (length > remaining) break
    remaining -= length
    result.unshift({ role: 'user', content: turn.question }, { role: 'assistant', content: turn.answer })
  }
  return result
}

export function citationExtension(sourceIds) {
  const valid = new Set(sourceIds.map(String))
  return {
    name: 'chatCitation', level: 'inline',
    start: (src) => src.indexOf('['),
    tokenizer(src) {
      const match = /^\[([1-9]\d{0,2})\](?!\()/.exec(src)
      if (match && valid.has(match[1])) return { type: 'chatCitation', raw: match[0], id: match[1] }
    },
    renderer: (token) => `<button type="button" class="chat-citation" data-citation="${token.id}" aria-label="查看来源 ${token.id}">[${token.id}]</button>`,
  }
}
