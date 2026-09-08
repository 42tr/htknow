import { readFileSync } from 'node:fs'
import vm from 'node:vm'
import assert from 'node:assert/strict'
import test from 'node:test'

// Exercise the component's actual data-loading code without a browser or canvas renderer.
function component(api = {}) {
  const source = readFileSync(new URL('./GraphVisualization.vue', import.meta.url), 'utf8')
    .match(/<script setup>([\s\S]*?)<\/script>/)[1].replace(/^import .*$/gm, '')
  const sandbox = {
    api, console, AbortController, Math, Set, Map,
    ref: value => ({ value }), computed: get => ({ get value() { return get() } }),
    defineProps: spec => Object.fromEntries(Object.entries(spec).map(([key, value]) => [key, value.default])),
    watch() {}, onMounted() {}, onBeforeUnmount() {},
  }
  vm.runInNewContext(source + '\nglobalThis.subject = { mergeSubgraph, loadGraphData, expandNode, allNodes, allEdges, truncated, loadError, canvas, loading };', sandbox)
  sandbox.subject.canvas.value = { width: 800, height: 600 }
  return sandbox.subject
}
const node = id => ({ id, name: `实体${id}`, entity_type: '概念' })
const edge = (id, source, target, type = '相关') => ({ id, source_id: source, target_id: target, relation_type: type })

test('preserves directed parallel relations and distinct document evidence', () => {
  const c = component()
  c.mergeSubgraph({ nodes: [node(1), node(2)], matched_ids: [1], truncated: false,
    edges: [edge(1, 1, 2, '负责'), edge(2, 2, 1, '依赖'), edge(3, 1, 2, '管理'), edge(4, 1, 2, '负责')] }, true)
  assert.equal(c.allEdges.value.length, 4)
  assert.equal(c.allEdges.value[1].source.id, 2)
  assert.equal(c.allEdges.value[1].target.id, 1)
  c.mergeSubgraph({ nodes: [node(1), node(2)], matched_ids: [1], truncated: false, edges: [edge(1, 1, 2, '负责')] })
  assert.equal(c.allEdges.value.length, 4)
})

test('limits the accumulated graph and never retains dangling edges', () => {
  const c = component()
  c.mergeSubgraph({ nodes: Array.from({ length: 250 }, (_, i) => node(i + 1)), matched_ids: [], truncated: false,
    edges: Array.from({ length: 500 }, (_, i) => edge(i + 1, 1, i % 249 + 2)) }, true)
  assert.equal(c.allNodes.value.length, 200)
  assert.ok(c.allEdges.value.length <= 400)
  const ids = new Set(c.allNodes.value.map(n => n.id))
  assert.ok(c.allEdges.value.every(e => ids.has(e.source.id) && ids.has(e.target.id)))
  assert.equal(c.truncated.value, true)
})

test('one subgraph request per load and late responses cannot overwrite a new scope', async () => {
  const requests = []
  const c = component({ getSubgraph: () => new Promise(resolve => requests.push(resolve)) })
  const first = c.loadGraphData()
  const second = c.loadGraphData()
  assert.equal(requests.length, 2)
  requests[1]({ nodes: [node(2)], matched_ids: [2], edges: [], truncated: false })
  await second
  requests[0]({ nodes: [node(1)], matched_ids: [1], edges: [], truncated: false })
  await first
  assert.equal(c.allNodes.value.length, 1)
  assert.equal(c.allNodes.value[0].id, 2)
  assert.equal(c.loading.value, false)
})
