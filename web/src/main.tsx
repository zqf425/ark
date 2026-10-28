import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ToastProvider } from './shared/ui'
import { I18nProvider } from './shared/i18n'
import './styles/global.css'
import './features/code-session/code-session.css'

const container = document.getElementById('root')
if (!container) throw new Error('#root 缺失，index.html 未正确加载')

createRoot(container).render(
  <StrictMode>
    <I18nProvider>
      <ToastProvider>
        <App />
      </ToastProvider>
    </I18nProvider>
  </StrictMode>
)
