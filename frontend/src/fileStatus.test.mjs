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
