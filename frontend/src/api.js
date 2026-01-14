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
  async searchFull(query, kbId = null, fileId = null) {
    let url = `${API_BASE}/search/full?query=${encodeURIComponent(query)}`
    if (kbId) {
      url += `&kb_id=${kbId}`
    }
    if (fileId) {
      url += `&file_id=${fileId}`
    }
    const response = await fetch(url, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('全文搜索失败')
    const data = await response.json()
    return data.results || []
  },

  // 知识库
  async getKnowledgeBases(parentId = null) {
    let url = `${API_BASE}/knowledge_base/`;
    const params = new URLSearchParams();
    // A null parentId fetches top-level KBs by default on the backend.
    if (parentId) {
      params.append('parent_id', parentId);
    }
    const queryString = params.toString();
    if (queryString) {
      url += `?${queryString}`;
    }
    const response = await fetch(url, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取知识库列表失败')
    return response.json()
  },

  async getKnowledgeBase(id) {
    const response = await fetch(`${API_BASE}/knowledge_base/${id}`, {
      headers: getHeaders(),
    });
    if (!response.ok) throw new Error('获取知识库详情失败');
    return response.json();
  },

  async createKnowledgeBase(data) { // data may include { name, description, parent_id }
    const response = await fetch(`${API_BASE}/knowledge_base/`, {
      method: 'POST',
      headers: getHeaders(),
      body: JSON.stringify(data),
    })
    if (!response.ok) throw new Error('创建知识库失败')
    return response.json()
  },

  async updateKnowledgeBase(id, data) { // data may include { name, description, parent_id, is_public }
    const response = await fetch(`${API_BASE}/knowledge_base/${id}`, {
      method: 'PUT',
      headers: getHeaders(),
      body: JSON.stringify(data),
    })
    if (!response.ok) throw new Error('更新知识库失败')
    return response.json()
  },

  async updateKnowledgeBasePublic(id, isPublic) {
    const response = await fetch(`${API_BASE}/knowledge_base/${id}/public`, {
      method: 'PUT',
      headers: getHeaders(),
      body: JSON.stringify({ is_public: isPublic }),
    })
    if (!response.ok) throw new Error('更新公开/私有状态失败')
    return response.json()
  },

  async deleteKnowledgeBase(id) {
    const response = await fetch(`${API_BASE}/knowledge_base/${id}`, {
      method: 'DELETE',
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('删除知识库失败')
    return
  },

  // 文件
  async uploadFiles(knowledgeBaseId, files, tags = [], isPublic = false, sliceType = 'smart') {
    const formData = new FormData()
    if (knowledgeBaseId) {
      formData.append('kb_id', knowledgeBaseId)
    }
    if (tags.length > 0) {
      formData.append('tags', JSON.stringify(tags))
    }
    formData.append('is_public', isPublic ? '1' : '0')
    formData.append('slice_type', sliceType)
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
    return
  },

  async getFiles(kbId, tag = null) {
    let url = `${API_BASE}/files/`
    const params = []
    if (kbId === null) {
      // 明确传递null时，获取未分配知识库的文件
      params.push('kb_id=null')
    } else if (kbId !== undefined) {
      // 传递具体的知识库ID
      params.push(`kb_id=${kbId}`)
    }
    if (tag) {
      params.push(`tag=${encodeURIComponent(tag)}`)
    }
    if (params.length > 0) {
      url += `?${params.join('&')}`
    }
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

  async updateFileTags(id, tags) {
    const response = await fetch(`${API_BASE}/files/${id}/tags`, {
      method: 'PUT',
      headers: getHeaders(),
      body: JSON.stringify({ tags }),
    })
    if (!response.ok) throw new Error('更新标签失败')
    return response.json()
  },

  async updateFilePublic(id, isPublic) {
    const response = await fetch(`${API_BASE}/files/${id}/public`, {
      method: 'PUT',
      headers: getHeaders(),
      body: JSON.stringify({ is_public: isPublic }),
    })
    if (!response.ok) throw new Error('更新公开/私有状态失败')
    return response.json()
  },

  async getFileSlices(fileId) {
    const response = await fetch(`${API_BASE}/files/${fileId}/slices`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取切片失败')
    return response.json()
  },

  // 知识图谱
  async searchEntities(query = null, entityType = null, kbId = null, limit = 100, fileId = null) {
    let url = `${API_BASE}/graph/entities?limit=${limit}`
    if (query) {
      url += `&q=${encodeURIComponent(query)}`
    }
    if (entityType) {
      url += `&entity_type=${encodeURIComponent(entityType)}`
    }
    if (kbId) {
      url += `&kb_id=${kbId}`
    }
    if (fileId) {
      url += `&file_id=${fileId}`
    }
    const response = await fetch(url, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('搜索实体失败')
    return response.json()
  },

  async getEntity(id) {
    const response = await fetch(`${API_BASE}/graph/entities/${id}`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取实体详情失败')
    return response.json()
  },

  async getGraphStats(kbId = null, fileId = null) {
    let url = `${API_BASE}/graph/stats`
    const params = []
    if (kbId) {
      params.push(`kb_id=${kbId}`)
    }
    if (fileId) {
      params.push(`file_id=${fileId}`)
    }
    if (params.length > 0) {
      url += '?' + params.join('&')
    }
    const response = await fetch(url, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取图谱统计失败')
    return response.json()
  },
}
