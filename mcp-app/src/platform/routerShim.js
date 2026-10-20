/**
 * vue-router 的空实现。
 *
 * KnowledgeSpace 从 cm 迁入时保留了 useRouter() 用法（非嵌入模式下头部导航
 * 跳「任务视图 / 流程管理」——那些页面属于 cm 宿主，MCP App 里不存在），
 * vite 把 vue-router 别名到这里，保持组件代码零改动。
 */
export function useRouter() {
  return {
    push() {},
    replace() {},
    back() {},
  };
}

export function useRoute() {
  return { path: "/", query: {}, params: {} };
}
