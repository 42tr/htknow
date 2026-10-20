/**
 * cm store 的适配实现。
 *
 * 从 cm 迁入的组件通过 useAuthStore() / useSystemConfigStore() 读取用户与系统信息，
 * 这里用 identity 上下文构造同形状的对象，组件代码零改动。
 */
import { reactive } from "vue";
import { resolveIdentity } from "@/platform/identity";

const identity = resolveIdentity();

const authState = reactive({
  user: {
    id: identity.userId,
    username: identity.userName || identity.userId,
    full_name: identity.userName || identity.userId,
    is_superuser: identity.role === "admin",
    role_codes: identity.role === "admin" ? ["super_admin"] : [identity.role],
  },
});

export function useAuthStore() {
  return {
    get user() {
      return authState.user;
    },
    // cm 里这个方法异步拉取登录态；MCP App 的身份在启动时已确定。
    async ensureInitialized() {},
  };
}

export function useSystemConfigStore() {
  return {
    systemInfo: {
      appLogo: "",
      appName: "HTKnow 知识空间",
      appVersion: "MCP",
    },
  };
}
