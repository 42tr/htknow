const API_BASE = '/api/v1/knowledge'

// 用户认证信息（实际应用中应该从登录获取）
const USER_ID = 'user1'
const ROLE = 'admin'

const getHeaders = (contentType = true) => {
  const headers = {
    'x-user-id': USER_ID,
    'x-role': ROLE,
  }
  if (contentType) {
    headers['Content-Type'] = 'application/json'
  }
  return headers
}

export const api = {
  // 搜索
  async search(query) {
    const response = await fetch(`${API_BASE}/search/?q=${encodeURIComponent(query)}`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('搜索失败')
    return response.json()
  },

  // 知识库
  async getKnowledgeBases() {
    const response = await fetch(`${API_BASE}/knowledge_base/`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取知识库失败')
    return response.json()
  },

  async createKnowledgeBase(data) {
    const response = await fetch(`${API_BASE}/knowledge_base/`, {
      method: 'POST',
      headers: getHeaders(),
      body: JSON.stringify(data),
    })
    if (!response.ok) throw new Error('创建知识库失败')
    return response.json()
  },

  async updateKnowledgeBase(id, data) {
    const response = await fetch(`${API_BASE}/knowledge_base/${id}`, {
      method: 'PUT',
      headers: getHeaders(),
      body: JSON.stringify(data),
    })
    if (!response.ok) throw new Error('更新知识库失败')
    return response.json()
  },

  async deleteKnowledgeBase(id) {
    const response = await fetch(`${API_BASE}/knowledge_base/${id}`, {
      method: 'DELETE',
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('删除知识库失败')
    return response.json()
  },

  // 文件
  async uploadFiles(knowledgeBaseId, files) {
    const formData = new FormData()
    formData.append('kb_id', knowledgeBaseId)
    for (const file of files) {
      formData.append('file', file)
    }

    const response = await fetch(`${API_BASE}/files/`, {
      method: 'POST',
      headers: getHeaders(false),
      body: formData,
    })
    if (!response.ok) throw new Error('上传文件失败')
    return response.json()
  },

  async deleteFile(id) {
    const response = await fetch(`${API_BASE}/files/${id}`, {
      method: 'DELETE',
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('删除文件失败')
    return response.json()
  },
}
