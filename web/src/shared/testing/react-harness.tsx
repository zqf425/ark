/**
 * Test-only React harness.
 *
 * Tests mount the *real* providers, so a panel is exercised with the same i18n
 * and toast wiring the app uses; the harness only supplies browser APIs jsdom
 * does not implement. Runtime I/O never happens here: callers mock the
 * `features/<x>/api` module of the unit under test instead.
 */
import { act, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { I18nProvider } from '../i18n'
import { ToastProvider } from '../ui'

/**
 * Browser APIs jsdom does not provide for these components.
 *
 * `I18nProvider` reads `matchMedia` for the dark-mode preference and persists
 * its preference to `localStorage`. jsdom only exposes a working `localStorage`
 * for an `http(s)` document, so the harness installs an in-memory one instead of
 * depending on the test document's origin. `IS_REACT_ACT_ENVIRONMENT` is what
 * tells React 18 that the test drives renders through `act`.
 */
export function installBrowserStubs(): void {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false
  })) as unknown as typeof window.matchMedia
  Object.defineProperty(window, 'localStorage', { value: createMemoryStorage(), configurable: true })
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
}

function createMemoryStorage(): Storage {
  const entries = new Map<string, string>()
  return {
    get length() {
      return entries.size
    },
    clear: () => entries.clear(),
    getItem: (key: string) => entries.get(key) ?? null,
    key: (index: number) => [...entries.keys()][index] ?? null,
    removeItem: (key: string) => {
      entries.delete(key)
    },
    setItem: (key: string, value: string) => {
      entries.set(key, value)
    }
  }
}

export type ReactHarness = {
  host: HTMLElement
  text: () => string
  render: (node: ReactNode) => Promise<void>
  unmount: () => Promise<void>
}

export async function mountWithProviders(node: ReactNode): Promise<ReactHarness> {
  const host = document.createElement('div')
  document.body.appendChild(host)
  const root = createRoot(host)

  const render = async (next: ReactNode): Promise<void> => {
    await act(async () => {
      root.render(
        <I18nProvider>
          <ToastProvider>{next}</ToastProvider>
        </I18nProvider>
      )
    })
  }

  await render(node)

  return {
    host,
    text: () => host.textContent ?? '',
    render,
    unmount: async () => {
      await act(async () => root.unmount())
      host.remove()
    }
  }
}

/**
 * Drain the promise chains a component starts (detail fetch, list fetch, then
 * its follow-up render). `act` around each turn keeps React's own bookkeeping
 * in sync with the state updates those chains produce.
 */
export async function settle(turns = 6): Promise<void> {
  for (let i = 0; i < turns; i += 1) {
    await act(async () => {
      await Promise.resolve()
    })
  }
}
