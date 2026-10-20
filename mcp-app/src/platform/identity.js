/**
 * MCP App 的身份上下文。
 *
 * htknow 的 REST 接口用 x-user-id / x-role / x-user-name 三个 header 鉴权。
 * 身份来源按优先级：
 * 1. window.__HTKNOW_MCP_IDENTITY__ —— 宿主（cm）或服务端在 HTML 中注入；
 * 2. URL query：?user_id=&role=&user_name= —— 开发与联调时用；
 * 3. 默认值（admin/1）—— 仅本地开发兜底。
 *
 * 经 cm 代理访问时，页面 URL 会带 access_token（cm 的登录令牌）：
 * 由 transport/download 放进 Authorization 头，cm 代理在服务端换成真实身份，
 * UI 自带的 x-user-* 头会被代理剥离，不构成越权面。
 */

function fromInjected() {
  const value = window.__HTKNOW_MCP_IDENTITY__;
  if (!value || value.userId === undefined || value.userId === null) return null;
  return {
    userId: String(value.userId),
    role: value.role || "user",
    userName: value.userName || "",
  };
}

function fromQuery() {
  const params = new URLSearchParams(window.location.search);
  const userId = params.get("user_id");
  if (!userId) return null;
  return {
    userId,
    role: params.get("role") || "user",
    userName: params.get("user_name") || "",
  };
}

export function resolveIdentity() {
  return fromInjected() || fromQuery() || { userId: "1", role: "admin", userName: "mcp-dev" };
}

/**
 * htknow 约定：HTTP 头只能放 ASCII，中文名按 `b64:<base64>` 显式编码传输
 * （与 htknow 自带前端同一格式）。
 */
function encodeUserName(name) {
  if (!name) return "";
  if (/^[\x00-\x7F]*$/.test(name)) return name;
  const bytes = new TextEncoder().encode(name);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return `b64:${btoa(binary)}`;
}

export function knowledgeHeaders(identity = resolveIdentity()) {
  return {
    "x-user-id": identity.userId,
    "x-role": identity.role,
    "x-user-name": encodeUserName(identity.userName),
  };
}

export function resolveAccessToken() {
  return new URLSearchParams(window.location.search).get("access_token") || "";
}
