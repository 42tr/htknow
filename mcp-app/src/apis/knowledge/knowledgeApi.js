import { httpRequest } from "@/apis/_shared/requestBridge";
import { useAuthStore } from "@/stores/authStore";
import { knowledgeHeaders as platformKnowledgeHeaders } from "@/platform/identity";

const BASE_URL = "/api/v1/knowledge";

function pickKnowledgeRole(user) {
  if (!user) return "user";
  if (user.is_superuser) return "admin";
  const codes = Array.isArray(user.role_codes) ? user.role_codes : [];
  if (codes.includes("super_admin")) return "admin";
  return codes[0] || "user";
}

function knowledgeHeaders() {
  const authStore = useAuthStore();
  const user = authStore.user || {};
  // 用户名编码（非 ASCII 按 htknow 约定的 b64: 形式）由平台层统一处理。
  const headers = platformKnowledgeHeaders();
  return {
    "x-user-id": String(user.id || "1"),
    "x-user-name": headers["x-user-name"] || "user",
    "x-role": pickKnowledgeRole(user),
  };
}

function knowledgeRequest(options = {}) {
  return httpRequest({
    module: "knowledge",
    autoAuth: false,
    autoRefreshOn401: false,
    ...options,
    headers: {
      ...knowledgeHeaders(),
      ...(options.headers || {}),
    },
  });
}

export function listKnowledgeBases(params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/knowledge_base/`,
    params,
  });
}

export function getKnowledgeBase(id, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/knowledge_base/${id}`,
    params,
  });
}

export function listKnowledgeBaseFiles(id, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/knowledge_base/${id}/files`,
    params,
  });
}

export function getKnowledgeBaseTags(id, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/knowledge_base/${id}/tags`,
    params,
  });
}

export function createKnowledgeBase(payload) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/knowledge_base/`,
    body: payload,
  });
}

export function updateKnowledgeBase(id, payload) {
  return knowledgeRequest({
    method: "PUT",
    url: `${BASE_URL}/knowledge_base/${id}`,
    body: payload,
  });
}

export function deleteKnowledgeBase(id) {
  return knowledgeRequest({
    method: "DELETE",
    url: `${BASE_URL}/knowledge_base/${id}`,
  });
}

export function reparseKnowledgeBase(id) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/knowledge_base/${id}/reparse`,
  });
}

export function uploadKnowledgeFiles({ files, kbId, tags = [], isPublic = false, immediateParse = true, sync = false }) {
  const formData = new FormData();
  for (const file of files) {
    formData.append("file", file);
  }
  formData.append("kb_id", String(kbId));
  formData.append("tags", JSON.stringify(tags));
  formData.append("is_public", String(Boolean(isPublic)));
  formData.append("immediate_parse", String(Boolean(immediateParse)));
  formData.append("sync", String(Boolean(sync)));

  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/files/`,
    body: formData,
  });
}

export function deleteKnowledgeFile(id) {
  return knowledgeRequest({
    method: "DELETE",
    url: `${BASE_URL}/files/${id}`,
  });
}

export function getKnowledgeFile(id, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${id}`,
    params,
  });
}

export function getKnowledgeFileSlices(id, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${id}/slices`,
    params,
  });
}

export function getKnowledgeSliceHighlightPage(fileId, sliceId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${fileId}/slices/${sliceId}/highlight-page`,
  });
}

export function getKnowledgeSliceHighlight(fileId, sliceId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${fileId}/slices/${sliceId}/highlight`,
  });
}

export function getKnowledgeExcelData(fileId, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${fileId}/excel-data`,
    params,
  });
}

export function updateKnowledgeFile(id, payload) {
  return knowledgeRequest({
    method: "PUT",
    url: `${BASE_URL}/files/${id}`,
    body: payload,
  });
}

export function downloadKnowledgeFile(id) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${id}/download`,
    responseType: "blob",
  });
}

export function downloadHighlightedPdf(id, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${id}/highlighted-pdf`,
    params,
    responseType: "blob",
  });
}

export function searchKnowledge(params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/search/`,
    params,
  });
}

export function getArchiveEntries(fileId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${fileId}/archive-entries`,
  });
}

export function extractArchive(fileId, password = null) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/files/${fileId}/archive-extract`,
    body: { password },
  });
}

export function downloadArchiveEntry(fileId, entryPath) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/files/${fileId}/archive-download`,
    params: { path: entryPath },
    responseType: "blob",
  });
}

export function buildKnowledgeFileDownloadUrl(id) {
  return `${BASE_URL}/files/${id}/download`;
}

export function buildArchiveEntryDownloadUrl(fileId, entryPath) {
  const encoded = encodeURIComponent(entryPath);
  return `${BASE_URL}/files/${fileId}/archive-download?path=${encoded}`;
}

// ── Wiki API ──────────────────────────────────────────────────────

export function getWikiIndex(kbId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/index`,
    params: { kb_id: kbId },
  });
}

export function getWikiPage(kbId, slug) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/page`,
    params: { kb_id: kbId, slug },
  });
}

export function listWikiPages(kbId, params = {}) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/pages`,
    params: { kb_id: kbId, ...params },
  });
}

export function searchWikiPages(kbId, q, limit = 20) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/search`,
    params: { kb_id: kbId, q, limit },
  });
}

export function getWikiStatus(kbId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/status`,
    params: { kb_id: kbId },
  });
}

export function getWikiConfig(kbId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/config`,
    params: { kb_id: kbId },
  });
}

export function updateWikiConfig(payload) {
  return knowledgeRequest({
    method: "PUT",
    url: `${BASE_URL}/wiki/config`,
    body: payload,
  });
}

export function rebuildWiki(kbId, fileId = null) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/wiki/rebuild`,
    body: { kb_id: kbId, file_id: fileId },
  });
}

export function updateWikiPage(payload) {
  return knowledgeRequest({
    method: "PUT",
    url: `${BASE_URL}/wiki/page`,
    body: payload,
  });
}

export function createWikiPage(payload) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/wiki/page`,
    body: payload,
  });
}

export function deleteWikiPage(kbId, slug) {
  return knowledgeRequest({
    method: "DELETE",
    url: `${BASE_URL}/wiki/page`,
    body: { kb_id: kbId, slug },
  });
}

export function listWikiRevisions(kbId, slug, limit = 50) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/revisions`,
    params: { kb_id: kbId, slug, limit },
  });
}

export function getWikiRevision(kbId, slug, version) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/revision`,
    params: { kb_id: kbId, slug, version },
  });
}

export function revertWikiPage(kbId, slug, version) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/wiki/revert`,
    body: { kb_id: kbId, slug, version },
  });
}

export function lintWiki(kbId) {
  return knowledgeRequest({
    method: "GET",
    url: `${BASE_URL}/wiki/lint`,
    params: { kb_id: kbId },
  });
}

export function rebuildWikiLinks(kbId) {
  return knowledgeRequest({
    method: "POST",
    url: `${BASE_URL}/wiki/rebuild-links`,
    body: { kb_id: kbId },
  });
}
