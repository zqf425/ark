import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ToastProvider } from './shared/ui'
import './styles/global.css'
import './features/code-session/code-session.css'

const container = document.getElementById('root')
if (!container) throw new Error('#root 缺失，index.html 未正确加载')

createRoot(container).render(
  <StrictMode>
    <ToastProvider>
      <App />
    </ToastProvider>
  </StrictMode>
)
