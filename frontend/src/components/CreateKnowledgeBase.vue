<script setup>
import { ref, defineProps } from 'vue'
import { api } from '../api'

const props = defineProps({
  parentId: {
    type: Number,
    default: null
  }
})

const emit = defineEmits(['created'])

const showModal = ref(false)
const name = ref('')
const description = ref('')
const kbType = ref('analysis')
const isPublic = ref(false)
const creating = ref(false)
const error = ref('')

const handleCreate = async () => {
  if (!name.value.trim()) {
    error.value = '请输入知识库名称'
    return
  }

  creating.value = true
  error.value = ''

  try {
    await api.createKnowledgeBase({
      name: name.value.trim(),
      description: description.value.trim(),
      kb_type: kbType.value,
      parent_id: props.parentId,
      is_public: isPublic.value
    })
    showModal.value = false
    name.value = ''
    description.value = ''
    kbType.value = 'analysis'
    isPublic.value = false
    emit('created')
  } catch (e) {
    error.value = e.message
  } finally {
    creating.value = false
  }
}

const closeModal = () => {
  showModal.value = false
  name.value = ''
  description.value = ''
  kbType.value = 'analysis'
  error.value = ''
}
</script>

<template>
  <div>
    <button
      @click="showModal = true"
      class="px-4 py-2.5 bg-gradient-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 transition-all duration-200 shadow-sm flex items-center gap-2"
    >
      <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 4v16m8-8H4" />
      </svg>
      新建知识库
    </button>

    <!-- Modal -->
    <Teleport to="body">
      <div
        v-if="showModal"
        class="fixed inset-0 z-50 flex items-center justify-center p-4"
      >
        <!-- Backdrop -->
        <div
          @click="closeModal"
          class="absolute inset-0 bg-black/50 backdrop-blur-sm"
        ></div>

        <!-- Modal Content -->
        <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-md p-6">
          <div class="flex items-center justify-between mb-5">
            <h3 class="text-lg font-semibold text-slate-800">新建知识库</h3>
            <button
              @click="closeModal"
              class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-all"
            >
              <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <div class="space-y-4">
            <div>
              <label class="block text-sm font-medium text-slate-700 mb-1.5">
                名称 <span class="text-red-500">*</span>
              </label>
              <input
                v-model="name"
                type="text"
                placeholder="输入知识库名称"
                class="w-full px-4 py-3 bg-slate-50 border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              />
            </div>

             <div>
               <label class="block text-sm font-medium text-slate-700 mb-1.5">
                 描述
               </label>
               <textarea
                 v-model="description"
                 placeholder="输入知识库描述（可选）"
                 rows="3"
                 class="w-full px-4 py-3 bg-slate-50 border border-slate-200 rounded-xl focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent resize-none"
               ></textarea>
             </div>

             <div>
               <label class="block text-sm font-medium text-slate-700 mb-2">
                 可见性
               </label>
               <div class="flex gap-4">
                 <label
                   :class="[
                     'flex-1 flex items-center gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
                     !isPublic ? 'border-blue-500 bg-blue-50' : 'border-slate-200 hover:border-slate-300'
                   ]"
                 >
                   <input type="radio" v-model="isPublic" :value="false" class="w-4 h-4 text-blue-500" />
                   <div>
                     <div class="flex items-center gap-2 mb-1">
                       <span class="text-lg">🔒</span>
                       <span class="font-medium text-slate-800">私有</span>
                     </div>
                     <p class="text-xs text-slate-500">仅自己可见</p>
                   </div>
                 </label>
                 <label
                   :class="[
                     'flex-1 flex items-center gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
                     isPublic ? 'border-green-500 bg-green-50' : 'border-slate-200 hover:border-slate-300'
                   ]"
                 >
                   <input type="radio" v-model="isPublic" :value="true" class="w-4 h-4 text-green-500" />
                   <div>
                     <div class="flex items-center gap-2 mb-1">
                       <span class="text-lg">🌐</span>
                       <span class="font-medium text-slate-800">公开</span>
                     </div>
                     <p class="text-xs text-slate-500">所有人可见</p>
                   </div>
                 </label>
               </div>
             </div>

             <div>
               <label class="block text-sm font-medium text-slate-700 mb-2">
                 类型
               </label>
               <div class="flex gap-4">
                 <label
                   :class="[
                     'flex-1 flex items-center gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
                     kbType === 'analysis' ? 'border-indigo-500 bg-indigo-50' : 'border-slate-200 hover:border-slate-300'
                   ]"
                 >
                   <input type="radio" v-model="kbType" value="analysis" class="w-4 h-4 text-indigo-500" />
                   <div>
                     <div class="flex items-center gap-2 mb-1">
                       <span class="text-lg">🧠</span>
                       <span class="font-medium text-slate-800">分析型</span>
                     </div>
                     <p class="text-xs text-slate-500">解析并切片，支持搜索与知识图谱</p>
                   </div>
                 </label>
                 <label
                   :class="[
                     'flex-1 flex items-center gap-3 p-4 rounded-xl border-2 cursor-pointer transition-all',
                     kbType === 'storage' ? 'border-amber-500 bg-amber-50' : 'border-slate-200 hover:border-slate-300'
                   ]"
                 >
                   <input type="radio" v-model="kbType" value="storage" class="w-4 h-4 text-amber-500" />
                   <div>
                     <div class="flex items-center gap-2 mb-1">
                       <span class="text-lg">🗄️</span>
                       <span class="font-medium text-slate-800">存储型</span>
                     </div>
                     <p class="text-xs text-slate-500">仅存储文件，不进行解析</p>
                   </div>
                 </label>
               </div>
             </div>

            <p v-if="error" class="text-sm text-red-500">{{ error }}</p>

            <div class="flex gap-3 pt-2">
              <button
                @click="closeModal"
                class="flex-1 py-3 bg-slate-100 text-slate-700 rounded-xl font-medium hover:bg-slate-200 transition-colors"
              >
                取消
              </button>
              <button
                @click="handleCreate"
                :disabled="creating"
                class="flex-1 py-3 bg-gradient-to-r from-blue-500 to-indigo-600 text-white rounded-xl font-medium hover:from-blue-600 hover:to-indigo-700 transition-all disabled:opacity-50 disabled:cursor-not-allowed"
              >
                <span v-if="creating">创建中...</span>
                <span v-else>创建</span>
              </button>
            </div>
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>
