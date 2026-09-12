import { createApp } from 'vue'
import './style.css'
import App from './App.vue'

async function start() {
  if (new URLSearchParams(location.search).has('logged_out')) {
    const link = document.createElement('a')
    link.href = '/api/auth/login'
    link.textContent = '已退出当前应用，点击重新登录'
    document.getElementById('app').replaceChildren(link)
    return
  }
  const response = await fetch('/api/auth/me')
  if (response.status === 401) {
    window.location.assign('/api/auth/login')
    return
  }
  if (!response.ok) throw new Error('暂时无法验证登录状态，请稍后重试')
  const user = await response.json()
  createApp(App).provide('currentUser', user).mount('#app')
}
start().catch(error => { document.getElementById('app').textContent = error.message })
