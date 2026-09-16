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
  if (!state) return null
  let text = state[0]
  if (file.wiki_status === 'running') {
    const stages = {
      extracting: '提取条目', citations: '关联原文切片',
      summary: '生成文档摘要', pages: '生成页面', finishing: '收尾中',
    }
    const stage = stages[file.wiki_stage]
    if (stage) {
      text += ' · ' + stage
      const { wiki_completed: completed, wiki_total: total } = file
      let percent = null
      if (file.wiki_stage === 'extracting') percent = 5
      else if (file.wiki_stage === 'citations' && Number.isInteger(completed) && Number.isInteger(total) && total > 0) percent = 5 + Math.round(25 * Math.min(completed / total, 1))
      else if (file.wiki_stage === 'summary') percent = 30
      else if (file.wiki_stage === 'pages' && Number.isInteger(completed) && Number.isInteger(total) && total > 0) percent = 30 + Math.round(65 * Math.min(completed / total, 1))
      else if (file.wiki_stage === 'finishing') percent = 95
      if (percent != null) text += ` · ${percent}%`
      if (['citations', 'summary', 'pages'].includes(file.wiki_stage) &&
          Number.isInteger(completed) && Number.isInteger(total) && total > 0 &&
          completed >= 0 && completed <= total) {
        text += `（${completed}/${total} ${file.wiki_stage === 'citations' ? '批' : '页'}）`
      }
    }
  }
  return { text, color: state[1], icon: state[2] }
}

export function fileStatusText(file) {
  return wikiStatusInfo(file)?.text || {
    '-1': '处理失败', 0: '待处理', 1: '已完成', 2: '处理中', 3: '不解析',
  }[file.processing_status ?? file.status] || '未知状态'
}
