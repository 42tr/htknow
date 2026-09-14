import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import assert from 'node:assert/strict'
import test from 'node:test'

import * as wikiDiff from '../wikiDiff.js'

// 在没有浏览器的情况下跑组件里真实的版本对比逻辑，diff 实现直接复用 wikiDiff.js。
function subject(initialProps = {}) {
  const source = readFileSync(new URL('./WikiRevisionDrawer.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1]
    .replace(/^import .*$/gm, '')
  const props = { kbId: 1, slug: 'concept/x', title: 'x', currentVersion: 0, currentContent: '', canEdit: true, ...initialProps }
  const sandbox = {
    console,
    ...wikiDiff,
    ref: (value) => ({ value }),
    computed: (get) => ({ get value() { return get() } }),
    defineProps: () => props,
    defineEmits: () => () => {},
    watch() {},
    onMounted() {},
  }
  vm.runInNewContext(`${source}\nglobalThis.subject = { sourceLabel, formatTime, marker, diff, selected };`, sandbox)
  return { exposed: sandbox.subject, props }
}

test('labels the author of each revision', () => {
  const { exposed } = subject()
  assert.equal(exposed.sourceLabel('pipeline'), '自动生成')
  assert.equal(exposed.sourceLabel('user'), '人工编辑')
  assert.equal(exposed.sourceLabel('revert'), '回滚')
  assert.equal(exposed.sourceLabel(''), '未知')
  assert.equal(exposed.sourceLabel('mystery'), 'mystery')
})

test('formats markers and timestamps defensively', () => {
  const { exposed } = subject()
  assert.equal(exposed.marker('insert'), '+')
  assert.equal(exposed.marker('delete'), '−')
  assert.equal(exposed.marker('equal'), ' ')
  assert.equal(exposed.formatTime(0), '')
  assert.match(exposed.formatTime(1760000000), /\d{4}/)
})

test('diffs the selected revision against the current content', () => {
  const { exposed, props } = subject({ currentContent: '一\n二\n三', currentVersion: 3 })
  exposed.selected.value = { version: 2, content: '一\n贰\n三' }
  const diff = exposed.diff.value
  assert.deepEqual(diff.rows.map((row) => row.type), ['equal', 'delete', 'insert', 'equal'])
  assert.deepEqual(diff.stat, { added: 1, removed: 1, equal: 2 })
  assert.equal(diff.identical, false)

  // 当前正文变化后，同一个已选版本的 diff 会跟着变（computed 每次现算）。
  props.currentContent = '一\n贰\n三'
  assert.equal(exposed.diff.value.identical, true)
  // 跨 realm 的数组不能用 deepEqual 比较，直接看长度。
  assert.equal(exposed.diff.value.rows.length, 0)
})

test('reports no diff before a revision is selected', () => {
  const { exposed } = subject({ currentContent: '一' })
  assert.equal(exposed.diff.value.identical, true)
  assert.equal(exposed.diff.value.rows.length, 0)
  assert.deepEqual(JSON.parse(JSON.stringify(exposed.diff.value.stat)), { added: 0, removed: 0, equal: 0 })
})

test('collapses long unchanged runs in the rendered diff', () => {
  const oldText = Array.from({ length: 30 }, (_, index) => `行${index + 1}`).join('\n')
  const newText = oldText.replace('行15', '行十五')
  const { exposed } = subject({ currentContent: newText, currentVersion: 9 })
  exposed.selected.value = { version: 8, content: oldText }
  const rows = exposed.diff.value.rows
  assert.equal(rows[0].type, 'gap')
  assert.equal(rows.at(-1).type, 'gap')
  assert.ok(rows.some((row) => row.type === 'delete' && row.value === '行15'))
  assert.ok(rows.some((row) => row.type === 'insert' && row.value === '行十五'))
})
