<script setup>
import { computed, onMounted, reactive, ref } from 'vue'
import { api } from '../api'

const loading = ref(true)
const saving = ref(false)
const error = ref('')
const success = ref('')
const activeEditor = ref('')
const form = reactive({
  mode: 'none', url: '', ocr_url: '', timeout_secs: 120, concurrency: 5,
  file_mode: 'mineru', custom_url: '', custom_reuse_url: '', mineru_url: '', office_convert_url: '', file_concurrency: 1, mineru_max_pages: 50,
  audio_url: '', audio_key: '', embedding_url: '', image_embedding_url: '', rerank_url: '',
})

const isCustom = computed(() => form.mode === 'custom')
const setActiveEditor = (name) => { activeEditor.value = name }
const clearActiveEditor = () => { activeEditor.value = '' }

const load = async () => {
  loading.value = true
  error.value = ''
  try {
    const [imageData, fileData, serviceData] = await Promise.all([api.getSettings('image_parse'), api.getSettings('file_parse'), api.getSettings('services')])
    const settings = imageData.settings || {}
    const fileSettings = fileData.settings || {}
    const serviceSettings = serviceData.settings || {}
    form.mode = settings['image_parse.mode']?.value || 'none'
    form.url = settings['image_parse.url']?.value || ''
    form.ocr_url = settings['image_parse.ocr_url']?.value || ''
    form.timeout_secs = settings['image_parse.timeout_secs']?.value || 120
    form.concurrency = settings['image_parse.concurrency']?.value || 5
    form.file_mode = fileSettings['file_parse.mode']?.value === 'custom' ? 'custom' : 'mineru'
    form.custom_url = fileSettings['file_parse.custom_url']?.value || ''
    form.custom_reuse_url = fileSettings['file_parse.custom_reuse_url']?.value || ''
    form.mineru_url = fileSettings['file_parse.mineru_url']?.value || ''
    form.office_convert_url = fileSettings['file_parse.office_convert_url']?.value || ''
    form.file_concurrency = fileSettings['file_parse.concurrency']?.value || 1
    form.mineru_max_pages = fileSettings['file_parse.mineru_max_pages']?.value ?? 50
    form.audio_url = serviceSettings['services.audio_transcription_url']?.value || ''
    form.embedding_url = serviceSettings['services.embedding_url']?.value || ''
    form.image_embedding_url = serviceSettings['services.image_embedding_url']?.value || ''
    form.rerank_url = serviceSettings['services.rerank_url']?.value || ''
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
  if (form.file_mode === 'custom' && !/^https?:\/\//i.test(form.custom_url.trim())) {
    error.value = '自定义文件解析接口地址必须以 http:// 或 https:// 开头'
    return
  }
  if (form.file_mode === 'mineru' && !/^https?:\/\//i.test(form.mineru_url.trim())) {
    error.value = 'MinerU 接口地址必须以 http:// 或 https:// 开头'
    return
  }
  if (!/^https?:\/\//i.test(form.office_convert_url.trim())) {
    error.value = 'Office 转 PDF 接口地址必须以 http:// 或 https:// 开头'
    return
  }
  if (form.custom_reuse_url.trim() && !/^https?:\/\//i.test(form.custom_reuse_url.trim())) {
    error.value = '文件解析复用接口地址必须以 http:// 或 https:// 开头'
    return
  }
  for (const [label, value] of [
    ['音频转写', form.audio_url],
    ['文本 Embedding', form.embedding_url],
    ['Rerank', form.rerank_url],
  ]) {
    if (!/^https?:\/\//i.test(value.trim())) {
      error.value = `${label}接口地址必须以 http:// 或 https:// 开头`
      return
    }
  }
  if (form.image_embedding_url.trim() && !/^https?:\/\//i.test(form.image_embedding_url.trim())) {
    error.value = '图片 Embedding 接口地址必须以 http:// 或 https:// 开头'
    return
  }
  saving.value = true
  try {
    const updates = {
      'image_parse.mode': form.mode,
      'image_parse.url': form.url.trim(),
      'image_parse.ocr_url': form.ocr_url.trim(),
      'image_parse.timeout_secs': Number(form.timeout_secs),
      'image_parse.concurrency': Number(form.concurrency),
      'file_parse.mode': form.file_mode,
      'file_parse.concurrency': Number(form.file_concurrency),
      'file_parse.office_convert_url': form.office_convert_url.trim(),
      'services.audio_transcription_url': form.audio_url.trim(),
      'services.embedding_url': form.embedding_url.trim(),
      'services.image_embedding_url': form.image_embedding_url.trim(),
      'services.rerank_url': form.rerank_url.trim(),
    }
    if (form.audio_key.trim()) {
      updates['services.audio_transcription_key'] = form.audio_key.trim()
    }
    if (form.file_mode === 'mineru') {
      updates['file_parse.mineru_url'] = form.mineru_url.trim()
      updates['file_parse.mineru_max_pages'] = Number(form.mineru_max_pages)
    } else {
      updates['file_parse.custom_url'] = form.custom_url.trim()
      updates['file_parse.custom_reuse_url'] = form.custom_reuse_url.trim()
    }
    await api.updateSettings(updates)
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
        <p class="mt-1 text-slate-500">配置图片处理和文件解析相关参数</p>
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
          <input v-model="form.ocr_url" @focus="setActiveEditor('ocr')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/ocr">
        </label>
        <div v-if="activeEditor === 'ocr'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
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
          <input v-model="form.url" @focus="setActiveEditor('image_custom')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/image-parse">
        </label>
        <div class="grid gap-4 md:grid-cols-2">
          <label class="text-sm font-medium text-slate-700">超时（秒）
            <input v-model.number="form.timeout_secs" min="1" max="600" type="number" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2">
          </label>
          <label class="text-sm font-medium text-slate-700">并发数
            <input v-model.number="form.concurrency" min="1" max="50" type="number" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2">
          </label>
        </div>
        <div v-if="activeEditor === 'image_custom'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
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

    <div v-if="!loading" class="rounded-xl border border-slate-200 bg-white p-6 shadow-sm">
      <h3 class="mb-4 text-lg font-semibold text-slate-800">文件解析</h3>
      <div class="grid gap-3 md:grid-cols-2">
        <label v-for="item in [
          { value: 'mineru', title: 'Mineru解析', desc: 'PDF 和图片直接使用 MinerU，Word/PPT 先转 PDF 再使用 MinerU，其他类型使用内置流程' },
          { value: 'custom', title: '自定义接口解析', desc: 'PDF 直接调用自定义接口，Word/PPT 先转 PDF 再调用自定义接口，其他类型使用内置流程' },
        ]" :key="item.value" class="cursor-pointer rounded-lg border p-4 transition" :class="form.file_mode === item.value ? 'border-blue-500 bg-blue-50' : 'border-slate-200 hover:border-slate-300'">
          <input v-model="form.file_mode" class="mr-2" type="radio" name="file-parse-mode" :value="item.value">
          <span class="font-medium text-slate-800">{{ item.title }}</span>
          <p class="mt-1 text-sm text-slate-500">{{ item.desc }}</p>
        </label>
      </div>

      <div v-if="form.file_mode === 'custom'" class="mt-6 space-y-4 border-t border-slate-200 pt-5">
        <label class="block text-sm font-medium text-slate-700">自定义解析接口
          <input v-model="form.custom_url" @focus="setActiveEditor('file_custom')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/parse">
        </label>
        <label class="block text-sm font-medium text-slate-700">解析结果复用接口（可选）
          <input v-model="form.custom_reuse_url" @focus="setActiveEditor('file_reuse')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/reuse">
        </label>
        <p class="text-xs text-slate-500">配置后，存在已有 PDF 内容时会优先尝试调用复用接口；复用失败则继续执行正常解析流程。</p>
        <div v-if="activeEditor === 'file_custom'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">自定义解析接口格式</p>
          <p class="mt-3 font-medium text-slate-700">请求格式</p>
          <pre class="mt-2 overflow-x-auto">POST {{ '{' }}url{{ '}' }}
Content-Type: multipart/form-data

file: 待解析文件</pre>
          <p class="mt-4 font-medium text-slate-700">输入格式</p>
          <p class="mt-2">上传原始 PDF、Word 或 PPT 文件，字段名为 <code>file</code>。</p>
          <p class="mt-4 font-medium text-slate-700">响应格式</p>
          <pre class="mt-2 overflow-x-auto">{{ '{' }}
  "code": 200,
  "message": "",
  "data": {{ '{' }}
    "slices": [{{ '{' }} "content": "文本内容" {{ '}' }}],
    "full_content": "完整文本",
    "summary": "摘要",
    "images": {{ '{' }} "image.png": "base64..." {{ '}' }}
  {{ '}' }}
{{ '}' }}</pre>
        </div>
      </div>

      <div v-if="form.file_mode === 'custom' && activeEditor === 'file_reuse'" class="mt-4 rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
        <p class="font-medium text-slate-700">解析结果复用接口格式</p>
        <p class="mt-3 font-medium text-slate-700">请求格式</p>
        <pre class="mt-2 overflow-x-auto">POST {{ '{' }}reuse_url{{ '}' }}
Content-Type: application/json

{{ '{' }}
  "pdf_contents": [
    {{ '{' }}
      "page_idx": 0,
      "bbox": "[0,0,100,100]",
      "text": "已有 PDF 内容",
      "text_level": 1,
      "img_path": null,
      "table_body": null
    {{ '}' }}
  ]
{{ '}' }}</pre>
        <p class="mt-4 font-medium text-slate-700">输入格式</p>
        <p class="mt-2">输入字段为 <code>pdf_contents</code>，内容是系统已有的 PDF 内容数组。</p>
        <p class="mt-4 font-medium text-slate-700">响应格式</p>
        <pre class="mt-2 overflow-x-auto">{{ '{' }}
  "code": 200,
  "message": "",
  "data": {{ '{' }}
    "slices": [{{ '{' }} "content": "复用后的切片内容" {{ '}' }}],
    "full_content": "完整文本",
    "summary": "摘要"
  {{ '}' }}
{{ '}' }}</pre>
      </div>

      <div v-if="form.file_mode === 'mineru'" class="mt-6 space-y-4 border-t border-slate-200 pt-5">
        <label class="block text-sm font-medium text-slate-700">MinerU 接口地址
          <input v-model="form.mineru_url" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="http://localhost:10001/file_parse">
        </label>
        <label class="text-sm font-medium text-slate-700">MinerU 最大页数
          <input v-model.number="form.mineru_max_pages" min="0" max="1000" type="number" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2">
          <span class="mt-1 block text-xs font-normal text-slate-500">设置为 0 表示不限制页数</span>
        </label>
      </div>
      <div class="mt-6 border-t border-slate-200 pt-5">
        <label class="block text-sm font-medium text-slate-700">文件解析并发数
          <input v-model.number="form.file_concurrency" min="1" max="50" type="number" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2">
        </label>
        <label class="mt-4 block text-sm font-medium text-slate-700">Office 转 PDF 接口
          <input v-model="form.office_convert_url" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="http://localhost:8003/convert">
        </label>
        <p class="mt-1 text-xs font-normal text-slate-500">Word/PPT 文件在进入 MinerU 或自定义解析接口前，会先转换为 PDF。</p>
      </div>
      <p class="mt-4 text-sm text-slate-500">文件解析配置将在后续文件处理或重新解析时生效。</p>
    </div>

    <div v-if="!loading" class="rounded-xl border border-slate-200 bg-white p-6 shadow-sm">
      <h3 class="mb-4 text-lg font-semibold text-slate-800">服务配置</h3>
      <div class="space-y-4">
        <label class="block text-sm font-medium text-slate-700">音频转写接口
          <input v-model="form.audio_url" @focus="setActiveEditor('audio')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/audio/transcriptions">
        </label>
        <label class="block text-sm font-medium text-slate-700">音频转写 API Key（留空保持不变）
          <input v-model="form.audio_key" type="password" autocomplete="new-password" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="请输入新的 API Key">
        </label>
        <div v-if="activeEditor === 'audio'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">请求格式</p>
          <pre class="mt-2 overflow-x-auto">POST {{ '{' }}url{{ '}' }}
Content-Type: multipart/form-data
Authorization: Bearer &lt;API Key&gt;

file: 音频文件</pre>
          <p class="mt-4 font-medium text-slate-700">响应格式</p>
          <pre class="mt-2 overflow-x-auto">{{ '{' }} "text": "转写文本", "language": "zh" {{ '}' }}</pre>
        </div>
        <label class="block text-sm font-medium text-slate-700">文本 Embedding 接口
          <input v-model="form.embedding_url" @focus="setActiveEditor('embedding')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/v1/embeddings">
        </label>
        <div v-if="activeEditor === 'embedding'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">请求格式</p>
          <pre class="mt-2 overflow-x-auto">POST {{ '{' }}url{{ '}' }}
Content-Type: application/json

{{ '{' }} "model": "模型名", "input": ["文本"] {{ '}' }}</pre>
          <p class="mt-4 font-medium text-slate-700">响应格式</p>
          <pre class="mt-2 overflow-x-auto">{{ '{' }} "data": [{{ '{' }} "embedding": [0.1, 0.2] {{ '}' }}] {{ '}' }}</pre>
        </div>
        <label class="block text-sm font-medium text-slate-700">图片 Embedding 接口（可选）
          <input v-model="form.image_embedding_url" @focus="setActiveEditor('image_embedding')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="未配置时不进行图片向量化">
        </label>
        <div v-if="activeEditor === 'image_embedding'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">请求格式</p>
          <pre class="mt-2 overflow-x-auto">POST {{ '{' }}url{{ '}' }}
Content-Type: multipart/form-data

file: 图片文件
text: 图片文件名或关联文本</pre>
          <p class="mt-4 font-medium text-slate-700">响应格式</p>
          <pre class="mt-2 overflow-x-auto">{{ '{' }} "data": [{{ '{' }} "embedding": [0.1, 0.2] {{ '}' }}] {{ '}' }}</pre>
        </div>
        <label class="block text-sm font-medium text-slate-700">Rerank 接口
          <input v-model="form.rerank_url" @focus="setActiveEditor('rerank')" @blur="clearActiveEditor" class="mt-1 w-full rounded-lg border border-slate-300 px-3 py-2" placeholder="https://example.com/v1/rerank">
        </label>
        <div v-if="activeEditor === 'rerank'" class="rounded-lg bg-slate-50 p-4 text-sm text-slate-600">
          <p class="font-medium text-slate-700">请求格式</p>
          <pre class="mt-2 overflow-x-auto">/v1/rerank:
{{ '{' }} "model": "模型名", "query": "查询文本", "documents": ["文档1"] {{ '}' }}

/rerank:
{{ '{' }} "query": "查询文本", "texts": ["文档1"] {{ '}' }}</pre>
          <p class="mt-4 font-medium text-slate-700">响应格式</p>
          <pre class="mt-2 overflow-x-auto">/v1/rerank:
{{ '{' }} "results": [{{ '{' }} "index": 0, "relevance_score": 0.9 {{ '}' }}] {{ '}' }}

/rerank:
[{{ '{' }} "index": 0, "score": 0.9 {{ '}' }}]</pre>
        </div>
      </div>
    </div>
  </section>
</template>
