// API 封装：Axios 实例 + 7 个端点
import axios from 'axios'

const http = axios.create({
  baseURL: '/api',
  timeout: 60000, // 同步可能较久
})

// 错误统一提取 message
http.interceptors.response.use(
  (res) => res.data,
  (err) => {
    const msg =
      err.response?.data?.error || err.response?.data?.message || err.message || '网络错误'
    return Promise.reject(new Error(msg))
  }
)

export const api = {
  folders: () => http.get('/folders'),
  messages: (params) => http.get('/messages', { params }),
  message: (id) => http.get(`/messages/${id}`),
  attachments: (id) => http.get(`/messages/${id}/attachments`),
  status: () => http.get('/status'),
  sync: () => http.post('/sync'),
}

export default api
