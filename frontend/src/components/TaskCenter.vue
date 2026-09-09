<script setup>
import { vDialog } from '../dialog'
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { api } from '../api'
import FileStatusSummary from './FileStatusSummary.vue'
import FileDetail from './FileDetail.vue'
const stats = ref({})
const index = ref(null)
const loading = ref(true)
const error = ref('')
const indexError = ref('')
const retrying = ref(false)
const message = ref('')
const selected = ref(null)
const confirmRetry = ref(false)
let timer,
  alive = true,
  busy = false
const refresh = async () => {
  if (busy) return
  busy = true
  const results = await Promise.allSettled([
    api.getFileStats(),
    api.getIndexRebuildStatus(),
  ])
  if (alive) {
    if (results[0].status === 'fulfilled') {
      stats.value = results[0].value
      error.value = ''
    } else error.value = results[0].reason.message
    if (results[1].status === 'fulfilled') {
      index.value = results[1].value
      indexError.value = ''
    } else indexError.value = results[1].reason.message
    loading.value = false
  }
  busy = false
}
const retryFailed = async () => {
  confirmRetry.value = false
  retrying.value = true
  message.value = ''
  try {
    const result = await api.reparseFailedFiles()
    message.value = `已提交 ${result.file_count || 0} 个文件重新处理`
    await refresh()
  } catch (e) {
    error.value = e.message
  } finally {
    retrying.value = false
  }
}
const openFile = async (file) => {
  try {
    selected.value = await api.getFile(file.id)
  } catch (e) {
    error.value = e.message
  }
}
onMounted(() => {
  refresh()
  timer = setInterval(() => {
    if (!document.hidden) refresh()
  }, 10000)
})
onBeforeUnmount(() => {
  alive = false
  clearInterval(timer)
})
</script>
<template>
  <section>
    <div class="page-title">
      <div>
        <span class="eyebrow">PROCESSING ACTIVITY</span>
        <h1>任务中心</h1>
        <p>查看资料处理状态，让失败的任务重新开始。</p>
      </div>
      <button class="secondary-button" @click="refresh">刷新状态</button>
    </div>
    <p class="search-help">全部知识库及未分配文件 · 每 10 秒自动更新</p>
    <p v-if="message" class="success-message" role="status">{{ message }}</p>
    <div class="results-workspace" :class="{ 'with-preview': selected }">
      <div class="min-w-0">
        <FileStatusSummary
          :stats="stats"
          :loading="loading"
          :error="error"
          :retry-failed-loading="retrying"
          title="文件处理"
          subtitle="点击处理中或失败状态，查看服务返回的文件记录"
          @retry="refresh"
          @reparse-failed="confirmRetry = true"
          @locate-file="openFile"
        />
        <section class="index-panel">
          <div class="panel-heading">
            <h2>搜索索引</h2>
            <span class="state-badge">{{
              {
                running: '重建中',
                completed: '已完成',
                failed: '重建失败',
                idle: '暂无任务',
              }[index?.status] || '暂无任务'
            }}</span>
          </div>
          <p v-if="indexError" class="inline-error">{{ indexError }}</p>
          <template v-else-if="index"
            ><p>{{ index.phase || '当前没有进行中的索引重建任务' }}</p>
            <p v-if="index.total_docs">
              已处理 {{ index.processed_docs || 0 }} /
              {{ index.total_docs }} 个文档
            </p>
            <progress
              v-if="index.total_docs"
              :value="index.processed_docs || 0"
              :max="index.total_docs"
            ></progress>
            <p v-if="index.error" class="inline-error">
              {{ index.error }}
            </p></template
          >
        </section>
      </div>
      <FileDetail v-if="selected" :file="selected" @close="selected = null" />
    </div>
    <div
      v-if="confirmRetry"
      class="upload-overlay"
      @click.self="confirmRetry = false"
    >
      <section
        v-dialog
        @keydown.esc="confirmRetry = false"
        class="confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="重新处理失败文件"
      >
        <h2>重新处理失败文件？</h2>
        <p>
          将重新提交所有知识库及未分配文件中可重试的失败文件。当前统计为
          {{ stats.failed || 0 }} 个。
        </p>
        <div class="dialog-actions">
          <button class="secondary-button" @click="confirmRetry = false">
            取消</button
          ><button class="primary-button" @click="retryFailed">
            确认重新处理
          </button>
        </div>
      </section>
    </div>
  </section>
</template>
