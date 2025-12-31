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
  async search(query, kbId = null, fileId = null) {
    let url = `${API_BASE}/search/?query=${encodeURIComponent(query)}`
    if (kbId) {
      url += `&kb_id=${kbId}`
    }
    if (fileId) {
      url += `&file_id=${fileId}`
    }
    const response = await fetch(url, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('搜索失败')
    const data = await response.json()
    return data.results || []
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
    if (knowledgeBaseId) {
      formData.append('kb_id', knowledgeBaseId)
    }
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

  async getFiles(kbId) {
    let url = `${API_BASE}/files/`
    if (kbId === null) {
      // 明确传递null时，获取未分配知识库的文件
      url += `?kb_id=null`
    } else if (kbId !== undefined) {
      // 传递具体的知识库ID
      url += `?kb_id=${kbId}`
    }
    // 不传参数时，获取所有文件
    const response = await fetch(url, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取文件列表失败')
    return response.json()
  },

  async getFile(id) {
    const response = await fetch(`${API_BASE}/files/${id}`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取文件失败')
    return response.json()
  },

  async updateFile(id, data) {
    const response = await fetch(`${API_BASE}/files/${id}`, {
      method: 'PUT',
      headers: getHeaders(),
      body: JSON.stringify(data),
    })
    if (!response.ok) throw new Error('更新文件失败')
    return response.json()
  },

  async getFileSlices(fileId) {
    const response = await fetch(`${API_BASE}/files/${fileId}/slices`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取切片失败')
    return response.json()
  },
}
