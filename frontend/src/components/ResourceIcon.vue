<script setup>
defineProps({
  type: {
    type: String,
    default: 'file',
  },
  open: {
    type: Boolean,
    default: false,
  },
})
</script>

<template>
  <svg
    v-if="type === 'folder' && open"
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M3 7.5h6l2 2h8.5a2 2 0 0 1 1.9 2.65l-2.1 6A2 2 0 0 1 17.4 19H5a2 2 0 0 1-1.94-1.52L2 13.2A3 3 0 0 1 4.91 9.5H6" />
    <path d="M3 10V6a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v1.5" />
  </svg>
  <svg
    v-else-if="type === 'folder'"
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z" />
  </svg>
  <svg
    v-else-if="type === 'library'"
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H11a3 3 0 0 1 3 3v14a3 3 0 0 0-3-3H6.5A2.5 2.5 0 0 0 4 19.5v-14Z" />
    <path d="M20 5.5A2.5 2.5 0 0 0 17.5 3H14v17a3 3 0 0 1 3-3h.5a2.5 2.5 0 0 1 2.5 2.5v-14Z" />
  </svg>
  <svg
    v-else-if="type === 'archive'"
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M5 3h14v5H5zM6 8h12v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V8Z" />
    <path d="M10 12h4" />
  </svg>
  <svg
    v-else
    aria-hidden="true"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M6 3h8l4 4v14H6z" />
    <path d="M14 3v5h5M9 13h6M9 17h6" />
  </svg>
</template>
