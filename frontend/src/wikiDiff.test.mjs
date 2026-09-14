import assert from 'node:assert/strict'
import test from 'node:test'

import { DIFF_DELETE, DIFF_EQUAL, DIFF_GAP, DIFF_INSERT, diffLines, foldDiff, splitLines, summarizeDiff } from './wikiDiff.js'

const types = (rows) => rows.map((row) => row.type)

test('identical texts produce only equal rows with matching line numbers', () => {
  const rows = diffLines('一\n二\n三', '一\n二\n三')
  assert.deepEqual(types(rows), [DIFF_EQUAL, DIFF_EQUAL, DIFF_EQUAL])
  assert.deepEqual(rows.map((row) => [row.oldNo, row.newNo]), [[1, 1], [2, 2], [3, 3]])
  assert.deepEqual(summarizeDiff(rows), { added: 0, removed: 0, equal: 3 })
})

test('a changed line becomes a delete plus an insert', () => {
  const rows = diffLines('一\n二\n三', '一\n贰\n三')
  assert.deepEqual(types(rows), [DIFF_EQUAL, DIFF_DELETE, DIFF_INSERT, DIFF_EQUAL])
  const removed = rows[1]
  const added = rows[2]
  assert.equal(removed.value, '二')
  assert.deepEqual([removed.oldNo, removed.newNo], [2, null])
  assert.equal(added.value, '贰')
  assert.deepEqual([added.oldNo, added.newNo], [null, 2])
  assert.deepEqual(summarizeDiff(rows), { added: 1, removed: 1, equal: 2 })
})

test('pure insertion keeps old line numbers stable', () => {
  const rows = diffLines('一\n三', '一\n二\n三')
  assert.deepEqual(types(rows), [DIFF_EQUAL, DIFF_INSERT, DIFF_EQUAL])
  assert.equal(rows[1].value, '二')
  assert.deepEqual(rows[2], { type: DIFF_EQUAL, value: '三', oldNo: 2, newNo: 3 })
})

test('pure deletion keeps new line numbers stable', () => {
  const rows = diffLines('一\n二\n三', '一\n三')
  assert.deepEqual(types(rows), [DIFF_EQUAL, DIFF_DELETE, DIFF_EQUAL])
  assert.deepEqual(rows[2], { type: DIFF_EQUAL, value: '三', oldNo: 3, newNo: 2 })
})

test('empty content diffs cleanly without a phantom line', () => {
  assert.deepEqual(splitLines(''), [])
  const rows = diffLines('', '新内容')
  assert.deepEqual(types(rows), [DIFF_INSERT])
  assert.deepEqual(diffLines('旧内容', ''), [{ type: DIFF_DELETE, value: '旧内容', oldNo: 1, newNo: null }])
  assert.deepEqual(diffLines('', ''), [])
})

test('fold keeps context around changes and collapses the rest', () => {
  const oldText = Array.from({ length: 20 }, (_, index) => `行${index + 1}`).join('\n')
  const newText = oldText.replace('行10', '行十')
  const folded = foldDiff(diffLines(oldText, newText), 2)
  assert.deepEqual(types(folded), [DIFF_GAP, DIFF_EQUAL, DIFF_EQUAL, DIFF_DELETE, DIFF_INSERT, DIFF_EQUAL, DIFF_EQUAL, DIFF_GAP])
  assert.deepEqual(folded[0], { type: DIFF_GAP, skipped: 7 })
  assert.deepEqual(folded.at(-1), { type: DIFF_GAP, skipped: 8 })
  assert.equal(folded[3].value, '行10')
  assert.equal(folded[4].value, '行十')
})

test('fold returns nothing when both versions are identical', () => {
  assert.deepEqual(foldDiff(diffLines('一\n二', '一\n二')), [])
})

test('oversized inputs fall back to a whole-block replace instead of hanging', () => {
  const make = (prefix, count) => Array.from({ length: count }, (_, index) => `${prefix}${index}`).join('\n')
  const rows = diffLines(make('旧', 2500), make('新', 2500))
  assert.equal(rows.length, 5000)
  assert.deepEqual(summarizeDiff(rows), { added: 2500, removed: 2500, equal: 0 })
})
