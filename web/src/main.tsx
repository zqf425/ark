import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { ToastProvider } from './shared/ui'
import { I18nProvider } from './shared/i18n'
import { useCodeSessionStore } from './features/code-session/state/store'
import './styles/global.css'
import './features/code-session/code-session.css'

const container = document.getElementById('root')
if (!container) throw new Error('#root is missing: index.html did not load correctly')

// Restore per-thread composer drafts before the first render, so every
// composer paints its persisted text instead of flashing empty after a reload
// (shell restart, arkd bounce, renderer crash — the "session finished and all
// drafts vanished" symptom).
useCodeSessionStore.getState().hydrateDrafts()

createRoot(container).render(
  <StrictMode>
    <I18nProvider>
      <ToastProvider>
        <App />
      </ToastProvider>
    </I18nProvider>
  </StrictMode>
)
