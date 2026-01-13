import { ref } from 'vue'

// This is a simple reactive store for cross-component state.
// For larger applications, consider using Pinia.

export const currentKb = ref({ id: null, name: '所有知识库' });

export const setCurrentKb = (kb) => {
  currentKb.value = kb;
};
