<script setup>
import { ref, onMounted } from 'vue';
import { api } from '../api';

const props = defineProps({
  show: Boolean,
});
const emit = defineEmits(['close', 'select']);

const currentParent = ref(null);
const breadcrumbs = ref([]);
const childrenKbs = ref([]);
const loading = ref(true);

const loadPath = async (kbId) => {
  if (!kbId) {
    breadcrumbs.value = [];
    return;
  }
  try {
    const data = await api.getKnowledgeBase(kbId);
    breadcrumbs.value = [...data.path, { id: data.id, name: data.name }];
  } catch (e) {
    console.error('Failed to load path', e);
  }
};

const loadChildren = async (parentId) => {
  loading.value = true;
  currentParent.value = parentId;
  if (parentId) {
    await loadPath(parentId);
  } else {
    breadcrumbs.value = [];
  }

  try {
    childrenKbs.value = await api.getKnowledgeBases(parentId);
  } catch (e) {
    console.error('Failed to load knowledge bases', e);
  } finally {
    loading.value = false;
  }
};

const selectKb = (kb) => {
  emit('select', kb);
  emit('close');
};

const navigate = (kbId) => {
  loadChildren(kbId);
};

onMounted(() => {
  loadChildren(null);
});
</script>

<template>
  <Teleport to="body">
    <div v-if="show" class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50 backdrop-blur-sm" @click.self="emit('close')">
      <div class="relative bg-white rounded-2xl shadow-xl w-full max-w-lg p-6">
        <h3 class="text-lg font-semibold text-slate-800 mb-4">Select a Knowledge Base</h3>

        <!-- Breadcrumbs -->
        <nav class="text-sm text-slate-500 mb-4 flex items-center gap-2">
          <span @click="navigate(null)" class="cursor-pointer hover:text-blue-500">Root</span>
          <template v-for="crumb in breadcrumbs" :key="crumb.id">
            <span>/</span>
            <span @click="navigate(crumb.id)" class="cursor-pointer hover:text-blue-500">{{ crumb.name }}</span>
          </template>
        </nav>

        <!-- List -->
        <div class="min-h-50 max-h-100 overflow-y-auto">
          <div v-if="loading" class="text-center p-8">Loading...</div>
          <ul v-else class="divide-y divide-slate-100">
            <li v-for="kb in childrenKbs" :key="kb.id" class="p-3 flex justify-between items-center group">
              <span class="text-slate-700">{{ kb.name }}</span>
              <div class="flex items-center gap-2">
                <button @click="selectKb(kb)" class="text-sm text-blue-500 hover:underline">Select</button>
                <button @click="navigate(kb.id)" class="text-sm text-slate-500 hover:underline opacity-50 group-hover:opacity-100">Open</button>
              </div>
            </li>
            <li v-if="!loading && childrenKbs.length === 0" class="text-center text-slate-400 p-8">No sub-folders.</li>
          </ul>
        </div>

        <!-- Current Folder Selection -->
        <div class="mt-4 pt-4 border-t border-slate-200">
          <button @click="selectKb(currentParent ? {id: currentParent, name: breadcrumbs[breadcrumbs.length-1]?.name || 'Root'} : null)" class="w-full py-2 bg-slate-100 rounded-lg hover:bg-slate-200">
            Select current folder: <span class="font-semibold">{{ currentParent ? (breadcrumbs[breadcrumbs.length-1]?.name || 'Root') : 'None (Top Level)' }}</span>
          </button>
        </div>

      </div>
    </div>
  </Teleport>
</template>
