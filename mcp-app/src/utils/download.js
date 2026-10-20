/**
 * 下载工具（MCP App 适配版）。
 *
 * 与 cm 版的差异：htknow 的下载端点在 x-user-id/x-role 鉴权之后，
 * `<a download>` 无法携带 header（cm 是靠 access_token query + 服务端代理注入），
 * 这里统一走 fetch（带身份头）→ blob → 浏览器保存。
 */
import { knowledgeHeaders, resolveAccessToken } from "@/platform/identity";

export function downloadBlob(blob, filename) {
  if (!(blob instanceof Blob)) {
    console.warn("[downloadBlob] expected Blob, got", typeof blob);
    return;
  }
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename || "download";
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}

export async function downloadFileByUrl(url, filename) {
  if (!url) {
    console.warn("[downloadFileByUrl] url is required");
    return;
  }
  const headers = knowledgeHeaders();
  const accessToken = resolveAccessToken();
  if (accessToken) {
    headers["Authorization"] = `Bearer ${accessToken}`;
  }
  const response = await fetch(url, { headers });
  if (!response.ok) {
    throw new Error(`下载失败（${response.status}）`);
  }
  downloadBlob(await response.blob(), filename);
}
