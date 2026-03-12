import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import './style.css'

const routes = [
  { path: '/', component: () => import('./views/Chat.vue') },
  { path: '/setup', component: () => import('./views/SetupWizard.vue') },
  // Legacy routes redirect to root (settings are now in the overlay panel)
  { path: '/chat', redirect: '/' },
  { path: '/config/:any(.*)', redirect: '/' },
  { path: '/skills', redirect: '/' },
  { path: '/agents', redirect: '/' },
  { path: '/cron', redirect: '/' },
  { path: '/knowledge', redirect: '/' },
  { path: '/logs', redirect: '/' },
]

const router = createRouter({
  history: createWebHistory(),
  routes,
})

createApp(App).use(router).mount('#app')
