// Wiki 版本对比用的行级 diff。
//
// 只在用户点开「历史」并选择某个版本时跑一次，规模是单页正文（几百到几千行）：
// 先削掉公共前后缀再做 LCS 动态规划，超出内存预算时退化成「整段替换」，
// 保证再大的页面也不会卡死浏览器。

export const DIFF_EQUAL = 'equal'
export const DIFF_INSERT = 'insert'
export const DIFF_DELETE = 'delete'
export const DIFF_GAP = 'gap'

// DP 单元上限：400 万个 Uint32 约 16MB，超过就退化。
const MAX_DP_CELLS = 4_000_000

export function splitLines(text) {
  const value = text ?? ''
  return value.length === 0 ? [] : value.split('\n')
}

const row = (type, value, oldNo, newNo) => ({ type, value, oldNo, newNo })

/**
 * 比较两段文本，返回按顺序排列的行：
 * `{ type: equal|insert|delete, value, oldNo, newNo }`，行号从 1 开始，缺失一侧为 null。
 */
export function diffLines(oldText, newText) {
  const a = splitLines(oldText)
  const b = splitLines(newText)

  let head = 0
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1
  let tailA = a.length
  let tailB = b.length
  while (tailA > head && tailB > head && a[tailA - 1] === b[tailB - 1]) {
    tailA -= 1
    tailB -= 1
  }

  const rows = []
  for (let index = 0; index < head; index += 1) rows.push(row(DIFF_EQUAL, a[index], index + 1, index + 1))
  rows.push(...diffMiddle(a.slice(head, tailA), b.slice(head, tailB), head))
  for (let index = 0; index < a.length - tailA; index += 1) {
    rows.push(row(DIFF_EQUAL, a[tailA + index], tailA + index + 1, tailB + index + 1))
  }
  return rows
}

function diffMiddle(a, b, offset) {
  if (a.length === 0 && b.length === 0) return []
  if (a.length * b.length > MAX_DP_CELLS) return replaceAll(a, b, offset)

  const width = b.length + 1
  const dp = new Uint32Array((a.length + 1) * width)
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      const here = i * width + j
      dp[here] = a[i] === b[j] ? dp[here + width + 1] + 1 : Math.max(dp[here + width], dp[here + 1])
    }
  }

  const rows = []
  let i = 0
  let j = 0
  let oldNo = offset + 1
  let newNo = offset + 1
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      rows.push(row(DIFF_EQUAL, a[i], oldNo, newNo))
      i += 1
      j += 1
      oldNo += 1
      newNo += 1
    } else if (dp[(i + 1) * width + j] >= dp[i * width + j + 1]) {
      rows.push(row(DIFF_DELETE, a[i], oldNo, null))
      i += 1
      oldNo += 1
    } else {
      rows.push(row(DIFF_INSERT, b[j], null, newNo))
      j += 1
      newNo += 1
    }
  }
  while (i < a.length) {
    rows.push(row(DIFF_DELETE, a[i], oldNo, null))
    i += 1
    oldNo += 1
  }
  while (j < b.length) {
    rows.push(row(DIFF_INSERT, b[j], null, newNo))
    j += 1
    newNo += 1
  }
  return rows
}

// 兜底路径：规模过大时不做 LCS，直接「删掉全部旧行 + 插入全部新行」。
function replaceAll(a, b, offset) {
  const rows = a.map((value, index) => row(DIFF_DELETE, value, offset + index + 1, null))
  b.forEach((value, index) => rows.push(row(DIFF_INSERT, value, null, offset + index + 1)))
  return rows
}

/** 统计增删行数。 */
export function summarizeDiff(rows) {
  let added = 0
  let removed = 0
  let equal = 0
  for (const item of rows) {
    if (item.type === DIFF_INSERT) added += 1
    else if (item.type === DIFF_DELETE) removed += 1
    else if (item.type === DIFF_EQUAL) equal += 1
  }
  return { added, removed, equal }
}

/**
 * 折叠未变化的长段，只保留每个变化点前后 `context` 行。
 * 两段之间插入 `{ type: 'gap', skipped }` 供 UI 显示「省略 N 行」。
 * 内容完全一致时返回空数组，UI 据此提示「无差异」。
 */
export function foldDiff(rows, context = 3) {
  const changed = rows.some((item) => item.type !== DIFF_EQUAL)
  if (!changed) return []

  const keep = new Array(rows.length).fill(false)
  rows.forEach((item, index) => {
    if (item.type === DIFF_EQUAL) return
    const from = Math.max(0, index - context)
    const to = Math.min(rows.length - 1, index + context)
    for (let cursor = from; cursor <= to; cursor += 1) keep[cursor] = true
  })

  const folded = []
  let skipped = 0
  rows.forEach((item, index) => {
    if (keep[index]) {
      if (skipped > 0) {
        folded.push({ type: DIFF_GAP, skipped })
        skipped = 0
      }
      folded.push(item)
      return
    }
    skipped += 1
  })
  if (skipped > 0) folded.push({ type: DIFF_GAP, skipped })
  return folded
}
