/**
 * 传输层适配：cm 的 knowledgeApi 经由 requestBridge 走到自研 request 客户端，
 * MCP App 里直接以 fetch 实现同一组选项（method/params/body/headers/responseType），
 * 返回 { data, status } 的 axios 风格响应，保持 knowledgeApi 与 view 层零改动。
 */

import { resolveAccessToken } from "./identity";

function appendParams(url, params) {
  if (!params || typeof params !== "object") return url;
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === "") continue;
    search.append(key, String(value));
  }
  const query = search.toString();
  if (!query) return url;
  return url + (url.includes("?") ? "&" : "?") + query;
}

export async function httpRequest(options = {}) {
  const { method = "GET", url, params, body, headers = {}, responseType } = options;
  if (!url) throw new Error("request options.url is required");

  const fetchOptions = { method, headers: { ...headers } };
  // cm 代理路径：携带 cm 令牌，由代理在服务端注入真实身份。
  const accessToken = resolveAccessToken();
  if (accessToken) {
    fetchOptions.headers["Authorization"] = `Bearer ${accessToken}`;
  }
  if (body !== undefined && body !== null) {
    if (
      typeof body === "string" ||
      (typeof FormData !== "undefined" && body instanceof FormData) ||
      (typeof URLSearchParams !== "undefined" && body instanceof URLSearchParams) ||
      (typeof Blob !== "undefined" && body instanceof Blob)
    ) {
      fetchOptions.body = body;
    } else {
      fetchOptions.headers["Content-Type"] = "application/json";
      fetchOptions.body = JSON.stringify(body);
    }
  }

  const response = await fetch(appendParams(url, params), fetchOptions);
  if (!response.ok) {
    let message = `请求失败（${response.status}）`;
    try {
      const text = await response.text();
      if (text) message = text;
    } catch {
      /* 保留默认 message */
    }
    const error = new Error(message);
    error.status = response.status;
    throw error;
  }

  const data = responseType === "blob" ? await response.blob() : await response.json();
  return { data, status: response.status };
}
