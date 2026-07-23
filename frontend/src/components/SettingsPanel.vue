<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { api } from '../api'

const loading = ref(true)
const saving = ref(false)
const error = ref('')
const success = ref('')
const form = reactive({ mode: 'none', url: '', ocr_url: '', timeout_secs: 120, concurrency: 5 })

const isCustom = computed(() => form.mode === 'custom')

const load = async () => {
  loading.value = true
  error.value = ''
  try {
    const data = await api.getSettings('image_parse')
    const settings = data.settings || {}
    form.mode = settings['image_parse.mode']?.value || 'none'
    form.url = settings['image_parse.url']?.value || ''
    form.ocr_url = settings['image_parse.ocr_url']?.value || ''
    form.timeout_secs = settings['image_parse.timeout_secs']?.value || 120
    form.concurrency = settings['image_parse.concurrency']?.value || 5
  } catch (err) {
    error.value = err.message || '读取配置失败'
  } finally {
    loading.value = false
  }
}

const save = async () => {
  error.value = ''
  success.value = ''
  if (isCustom.value && !/^https?:\/\//i.test(form.url.trim())) {
    error.value = '自定义接口地址必须以 http:// 或 https:// 开头'
    return
  }
  if (form.mode === 'ocr' && !/^https?:\/\//i.test(form.ocr_url.trim())) {
    error.value = 'OCR 接口地址必须以 http:// 或 https:// 开头'
    return
  }
  saving.value = true
  try {
    await api.updateSettings({
      'image_parse.mode': form.mode,
      'image_parse.url': form.url.trim(),
      'image_parse.ocr_url': form.ocr_url.trim(),
      'image_parse.timeout_secs': Number(form.timeout_secs),
      'image_parse.concurrency': Number(form.concurrency),
    })
    success.value = '配置已保存，新配置将在后续文件解析时生效'
  } catch (err) {
    error.value = err.message || '保存配置失败'
  } finally {
    saving.value = false
  }
}

onMounted(load)
</script>

<template>
  <section class="space-y-6">
    <div class="flex items-center justify-between">
      <div>
        <h2 class="text-2xl font-bold text-slate-800">系统配置</h2>
        <p class="mt-1 text-slate-500">配置图片在文档解析过程中的处理方式</p>
      </div>
      <button class="rounded-lg bg-slate-900 px-4 py-2 text-sm font-medium text-white disabled:opacity-50" :disabled="loading || saving" @click="save">
        {{ saving ? '保存中...' : '保存配置' }}
      </button>
    </div>

    <div v-if="loading" class="rounded-xl border border-slate-200 bg-white p-6 text-slate-500">正在读取配置...</div>
    <div v-else class="rounded-xl border border-slate-200 bg-white p-6 shadow-sm">
      <h3 class="mb-4 text-lg font-semibold text-slate-800">图片处理</h3>
      <div class="grid gap-3 md:grid-cols-3">
        <label v-for="item in [
          { value: 'ocr', title: 'OCR', desc: '识别图片中的文字' },
          { value: 'custom', title: '自定义接口', desc: '调用外部图片解析服务' },
          { value: 'none', title: '不解析', desc: '保留图片，不生成文本描述' },
        ]" :key="item.value" class="cursor-pointer rounded-lg border p-4 transition" :class="form.mode === item.value ? 'border-blue-500 bg-blue-50' : 'border-slate-200 hover:border-slate-300'">
          <input v-model="form.mode" class="mr-2" type="radio" name="image-parse-mode" :value="item.value">
          <span class="font-medium text-slate-800">{{ item.title }}</span>
          <p class="mt-1 text-sm text-slate-500">{{ item.desc }}</p>
        </label>
      </div>

      <div v-if="form.mode === 'ocr'" class="mt-6 space-y-4 border-t border-slate-200 pt-5">
        <label class="block text-sm font-medium text-slate-700">OCR 接口地址
          <input v-model="form.ocr_url" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/ocr">
        </label>
        <div class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">OCR 请求格式</p>
          <pre class="mt-2 overflow-x-auto">POST {{ '{' }}url{{ '}' }}
Content-Type: application/json

{{ '{' }} "figure_base64": "..." {{ '}' }}</pre>
          <p class="mt-4 font-medium text-slate-700">OCR 响应格式</p>
          <pre class="mt-2 overflow-x-auto">{{ '{' }} "data": "OCR 结果" {{ '}' }}</pre>
        </div>
      </div>

      <div v-if="isCustom" class="mt-6 space-y-4 border-t border-slate-200 pt-5">
        <label class="block text-sm font-medium text-slate-700">接口地址
          <input v-model="form.url" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/image-parse">
        </label>
        <div class="grid gap-4 md:grid-cols-2">
          <label class="text-sm font-medium text-slate-700">超时（秒）
            <input v-model.number="form.timeout_secs" min="1" max="600" type="number" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2">
          </label>
          <label class="text-sm font-medium text-slate-700">并发数
            <input v-model.number="form.concurrency" min="1" max="50" type="number" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2">
          </label>
        </div>
        <div class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">请求格式</p>
          <pre class="mt-2 overflow-x-auto">POST {{ '{' }}url{{ '}' }}
Content-Type: application/json

{{ '{' }} "content": null, "filename": "image.png", "image_base64": "..." {{ '}' }}</pre>
          <p class="mt-4 font-medium text-slate-700">推荐响应格式</p>
          <pre class="mt-2 overflow-x-auto">{{ '{' }} "description": "图片内容描述" {{ '}' }}</pre>
          <p class="mt-2">兼容字段：image_content、description、text、content、result，以及 data/result 嵌套结构。</p>
        </div>
      </div>

      <p v-if="error" class="mt-4 rounded-lg bg-red-50 p-3 text-sm text-red-700">{{ error }}</p>
      <p v-if="success" class="mt-4 rounded-lg bg-emerald-50 p-3 text-sm text-emerald-700">{{ success }}</p>
    </div>
  </section>
</template>
