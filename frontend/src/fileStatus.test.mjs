import test from 'node:test'
import assert from 'node:assert/strict'
import { fileStatusText, wikiStatusInfo } from './fileStatus.js'

test('parsed documents remain in the Wiki stage until generation completes', () => {
  for (const [wiki_status, label] of Object.entries({
    pending: '等待生成 Wiki', running: '正在生成 Wiki', retrying: 'Wiki 生成重试中', failed: 'Wiki 生成失败',
  })) {
    assert.equal(fileStatusText({ status: 1, wiki_status }), label)
    assert.equal(wikiStatusInfo({ status: 1, wiki_status }).text, label)
  }
  assert.equal(fileStatusText({ status: 1, wiki_status: 'completed' }), '已完成')
  assert.equal(fileStatusText({ status: 1, wiki_status: null }), '已完成')
  assert.equal(fileStatusText({ status: 1, wiki_status: 'skipped' }), '已完成（Wiki 无可用正文）')
})

test('parse failures, pending reparses and storage files take precedence over old Wiki results', () => {
  for (const [status, label] of [[-1, '处理失败'], [0, '待处理'], [2, '处理中'], [3, '不解析']]) {
    assert.equal(fileStatusText({ status, wiki_status: 'completed' }), label)
    assert.equal(wikiStatusInfo({ status, wiki_status: 'failed' }), null)
  }
})

test('Wiki progress displays real stage counts and hides stale progress outside running', () => {
  const file = { status: 1, wiki_status: 'running', wiki_stage: 'pages', wiki_completed: 3, wiki_total: 8 }
  assert.equal(fileStatusText(file), '正在生成 Wiki · 生成页面 · 54%（3/8 页）')
  assert.equal(fileStatusText({ ...file, wiki_stage: 'citations' }), '正在生成 Wiki · 关联原文切片 · 14%（3/8 批）')
  assert.equal(fileStatusText({ ...file, wiki_stage: 'extracting' }), '正在生成 Wiki · 提取条目 · 5%')
  assert.equal(fileStatusText({ ...file, wiki_stage: 'summary', wiki_completed: 0 }), '正在生成 Wiki · 生成文档摘要 · 30%（0/8 页）')
  assert.equal(fileStatusText({ ...file, wiki_stage: 'finishing' }), '正在生成 Wiki · 收尾中 · 95%')
  assert.equal(fileStatusText({ ...file, wiki_total: null }), '正在生成 Wiki · 生成页面')
  assert.equal(fileStatusText({ ...file, wiki_completed: 9 }), '正在生成 Wiki · 生成页面 · 95%')
  assert.equal(fileStatusText({ ...file, wiki_status: 'retrying' }), 'Wiki 生成重试中')
  assert.equal(fileStatusText({ ...file, wiki_status: 'completed' }), '已完成')
})
