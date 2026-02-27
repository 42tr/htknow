const API_BASE = '/api/v1/knowledge'

// 用户认证信息（实际应用中应该从登录获取）
const USER_ID = 'user1'
const USER_NAME = '42tr'
const ROLE = 'admin'

const getHeaders = (contentType = true) => {
  const headers = {
    'x-user-id': USER_ID,
    'x-user-name': USER_NAME,
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

  advancedSearchStream(params, handlers = {}) {
    const {
      query,
      kbId = null,
      fileId = null,
      maxSubQueries = 3,
      perQueryLimit = 5,
      contextChars = 2000,
      debug = false,
    } = params

    const searchParams = new URLSearchParams()
    searchParams.set('query', query)
    searchParams.set('max_sub_queries', maxSubQueries)
    searchParams.set('per_query_limit', perQueryLimit)
    searchParams.set('context_chars', contextChars)
    if (debug) searchParams.set('debug', 'true')
    if (kbId) searchParams.set('kb_id', kbId)
    if (fileId) searchParams.set('file_id', fileId)

    const controller = new AbortController()
    const url = `${API_BASE}/search/advanced/stream?${searchParams.toString()}`
    const headers = getHeaders(false)
    const decoder = new TextDecoder('utf-8')

    const parseEvent = (chunk) => {
      const lines = chunk.split('\n')
      let eventType = 'message'
      let dataLines = []
      for (const line of lines) {
        const trimmed = line.trim()
        if (trimmed.startsWith('event:')) {
          eventType = trimmed.slice(6).trim()
        } else if (trimmed.startsWith('data:')) {
          dataLines.push(trimmed.slice(5).trim())
        }
      }
      const dataStr = dataLines.join('\n')
      let parsedData = null
      if (dataStr) {
        try {
          parsedData = JSON.parse(dataStr)
        } catch (err) {
          parsedData = dataStr
        }
      }
      return { eventType, data: parsedData }
    }

    const streamPromise = (async () => {
      try {
        const response = await fetch(url, {
          headers,
          signal: controller.signal,
        })
        if (!response.ok) {
          throw new Error('高级搜索连接失败')
        }
        const reader = response.body.getReader()
        let buffer = ''
        while (true) {
          const { value, done } = await reader.read()
          if (done) break
          buffer += decoder.decode(value, { stream: true })
          let boundary
          while ((boundary = buffer.indexOf('\n\n')) !== -1) {
            const rawEvent = buffer.slice(0, boundary).trim()
            buffer = buffer.slice(boundary + 2)
            if (!rawEvent) continue
            const { eventType, data } = parseEvent(rawEvent)
            switch (eventType) {
              case 'status':
                handlers.onStatus?.(data)
                break
              case 'plan':
                handlers.onPlan?.(data)
                break
              case 'step':
                handlers.onStep?.(data)
                break
              case 'candidate':
                handlers.onCandidate?.(data)
                break
              case 'filtered':
                handlers.onFiltered?.(data)
                break
              case 'result':
                handlers.onResult?.(data)
                break
              case 'error':
                handlers.onErrorEvent?.(data)
                break
              case 'done':
                handlers.onDone?.()
                break
              default:
                handlers.onMessage?.({ eventType, data })
            }
          }
        }
        handlers.onComplete?.()
      } catch (err) {
        if (controller.signal.aborted) {
          handlers.onAbort?.()
        } else {
          handlers.onError?.(err)
        }
      } finally {
        handlers.onFinally?.()
      }
    })()

    return {
      cancel: () => controller.abort(),
      finished: streamPromise,
    }
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
  async searchImage(file, text = '', kbId = null, fileId = null) {
    const formData = new FormData()
    formData.append('file', file)
    formData.append('text', text)
    let url = `${API_BASE}/search/image`
    const params = []
    if (kbId) {
      params.push(`kb_id=${kbId}`)
    }
    if (fileId) {
      params.push(`file_id=${fileId}`)
    }
    if (params.length > 0) {
      url += `?${params.join('&')}`
    }
    const response = await fetch(url, {
      method: 'POST',
      headers: getHeaders(false),
      body: formData,
    })
    if (!response.ok) throw new Error('图片搜索失败')
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

  async deleteKnowledgeBase(id) {
    const response = await fetch(`${API_BASE}/knowledge_base/${id}`, {
      method: 'DELETE',
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('删除知识库失败')
    return
  },

  async reparseKnowledgeBases() {
    const response = await fetch(`${API_BASE}/knowledge_base/reparse`, {
      method: 'POST',
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('重新解析失败')
    return response.json()
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
    formData.append('is_public', isPublic ? 'true' : 'false')
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

  async getFileStats(options = {}) {
    const {
      kbId = undefined,
      includeDescendants = true,
      includeUnassigned = true,
    } = options
    const params = new URLSearchParams()
    if (kbId === null) {
      params.append('kb_id', 'null')
    } else if (kbId !== undefined) {
      params.append('kb_id', kbId)
    }
    if (!includeDescendants) {
      params.append('include_descendants', 'false')
    }
    if (!includeUnassigned) {
      params.append('include_unassigned', 'false')
    }
    const query = params.toString()
    const response = await fetch(
      `${API_BASE}/files/stats${query ? `?${query}` : ''}`,
      { headers: getHeaders() }
    )
    if (!response.ok) throw new Error('获取文件状态统计失败')
    return response.json()
  },

  async getFile(id) {
    const response = await fetch(`${API_BASE}/files/${id}`, {
      headers: getHeaders(),
    })
    if (!response.ok) throw new Error('获取文件失败')
    return response.json()
  },

  async downloadFile(id) {
    const response = await fetch(`${API_BASE}/files/${id}/download`, {
      headers: getHeaders(false),
    })
    if (!response.ok) throw new Error('下载文件失败')
    return response.blob()
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
