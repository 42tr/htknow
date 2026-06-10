<script setup>
import { ref, computed, watch } from 'vue'
import { api } from '../api'

const props = defineProps({
  kb: {
    type: Object,
    required: true,
  },
  show: {
    type: Boolean,
    default: false,
  },
})

const emit = defineEmits(['close'])

const permissions = ref([])
const loading = ref(false)
const saving = ref(false)
const error = ref('')

const newUserId = ref('')
const newPermission = ref('viewer')

const permissionOptions = [
  { value: 'viewer', label: '👁️ 只读', desc: '可查看、搜索、下载' },
  { value: 'editor', label: '✏️ 可写', desc: '可上传文件、重新解析' },
  { value: 'admin', label: '⚙️ 管理员', desc: '可修改属性、删除、管理权限' },
]

const permissionLabel = (perm) => {
  const opt = permissionOptions.find(o => o.value === perm)
  return opt ? opt.label : perm
}

const loadPermissions = async () => {
  if (!props.kb?.id) return
  loading.value = true
  error.value = ''
  try {
    permissions.value = await api.getKbPermissions(props.kb.id)
  } catch (e) {
    error.value = e.message
  } finally {
    loading.value = false
  }
}

const addPermission = async () => {
  if (!newUserId.value.trim()) {
    error.value = '请输入用户ID'
    return
  }
  saving.value = true
  error.value = ''
  try {
    await api.addKbPermission(props.kb.id, newUserId.value.trim(), newPermission.value)
    newUserId.value = ''
    newPermission.value = 'viewer'
    await loadPermissions()
  } catch (e) {
    error.value = e.message
  } finally {
    saving.value = false
  }
}

const removePermission = async (userId) => {
  if (!confirm(`确定要删除用户「${userId}」的权限吗？`)) return
  try {
    await api.removeKbPermission(props.kb.id, userId)
    await loadPermissions()
  } catch (e) {
    error.value = e.message
  }
}

watch(() => props.show, (val) => {
  if (val) loadPermissions()
})
</script>

<template>
  <Teleport to="body">
    <div
      v-if="show"
      class="fixed inset-0 z-50 flex items-center justify-center p-4"
    >
      <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" @click="emit('close')"></div>

      <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-lg p-6">
        <div class="flex items-center justify-between mb-5">
          <h3 class="text-lg font-semibold text-slate-800">
            📋 「{{ kb.name }}」权限管理
          </h3>
          <button @click="emit('close')" class="p-2 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-all">
            <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <div class="mb-4">
          <p class="text-sm text-slate-500">
            创建者：<span class="font-medium text-slate-700">{{ kb.user_id }}</span>
            <span class="ml-2 px-2 py-0.5 text-xs rounded-full bg-purple-50 text-purple-600 border border-purple-200">⚙️ 管理员</span>
          </p>
        </div>

        <!-- Add new permission -->
        <div class="bg-slate-50 rounded-xl p-4 mb-4">
          <label class="block text-sm font-medium text-slate-700 mb-2">添加成员</label>
          <div class="flex gap-2">
            <input
              v-model="newUserId"
              type="text"
              placeholder="用户ID"
              class="flex-1 px-3 py-2 bg-white border border-slate-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <select
              v-model="newPermission"
              class="px-3 py-2 bg-white border border-slate-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option v-for="opt in permissionOptions" :key="opt.value" :value="opt.value">
                {{ opt.label }}
              </option>
            </select>
            <button
              @click="addPermission"
              :disabled="saving"
              class="px-4 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors disabled:opacity-50"
            >
              {{ saving ? '...' : '添加' }}
            </button>
          </div>
        </div>

        <!-- Error -->
        <p v-if="error" class="text-sm text-red-500 mb-3">{{ error }}</p>

        <!-- Permission list -->
        <div class="max-h-64 overflow-y-auto">
          <div v-if="loading" class="text-center py-4 text-slate-500">加载中...</div>
          <div v-else-if="permissions.length === 0" class="text-center py-4 text-slate-400">暂无其他成员权限</div>
          <div v-else class="space-y-2">
            <div
              v-for="perm in permissions"
              :key="perm.user_id"
              class="flex items-center justify-between p-3 bg-slate-50 rounded-lg"
            >
              <div class="flex items-center gap-3">
                <div class="w-8 h-8 bg-blue-100 rounded-full flex items-center justify-center text-blue-600 font-medium text-sm">
                  {{ perm.user_id.charAt(0).toUpperCase() }}
                </div>
                <div>
                  <p class="text-sm font-medium text-slate-800">{{ perm.user_id }}</p>
                  <p class="text-xs text-slate-500">{{ permissionLabel(perm.permission) }}</p>
                </div>
              </div>
              <button
                @click="removePermission(perm.user_id)"
                class="p-1.5 text-slate-400 hover:text-red-500 hover:bg-red-50 rounded-lg transition-all"
              >
                <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                </svg>
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>
