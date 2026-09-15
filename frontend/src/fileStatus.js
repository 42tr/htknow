export function wikiStatusInfo(file) {
  if (file.status !== 1) return null
  const states = {
    pending: ['等待生成 Wiki', 'bg-amber-100 text-amber-700', '·'],
    running: ['正在生成 Wiki', 'bg-blue-100 text-blue-700', '·'],
    retrying: ['Wiki 生成重试中', 'bg-amber-100 text-amber-700', '·'],
    failed: ['Wiki 生成失败', 'bg-red-100 text-red-700', '✗'],
    skipped: ['已完成（Wiki 无可用正文）', 'bg-green-100 text-green-700', '✓'],
  }
  const state = states[file.wiki_status]
  return state ? { text: state[0], color: state[1], icon: state[2] } : null
}

export function fileStatusText(file) {
  return wikiStatusInfo(file)?.text || {
    '-1': '处理失败', 0: '待处理', 1: '已完成', 2: '处理中', 3: '不解析',
  }[file.processing_status ?? file.status] || '未知状态'
}
