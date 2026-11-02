// @vitest-environment jsdom
/**
 * The WeChat QR login dialog's rendering contract.
 *
 * Pinned here: the iLink payload is never shown as a bare string — an image
 * data URL renders as `<img>`, any other content (the login URL) is encoded
 * into an actual QR code, matching smart-ark's renderer. Without this the
 * dialog used to display a raw string the user could not scan.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { installBrowserStubs, mountWithProviders, settle, type ReactHarness } from '../../../shared/testing/react-harness'
import { startWeixinLogin, waitWeixinLogin } from '../api/im-api'
import { WeixinLoginDialog } from './WeixinLoginDialog'

vi.mock('../api/im-api', () => ({
  startWeixinLogin: vi.fn(),
  waitWeixinLogin: vi.fn(),
  // `useIm` refreshes the channel list after login side effects; without this
  // stub the hook silently lands in its error branch during tests.
  listImChannels: vi.fn().mockResolvedValue({ channels: [] })
}))

const startMock = vi.mocked(startWeixinLogin)
const waitMock = vi.mocked(waitWeixinLogin)

function pending() {
  return {
    status: 'pending' as const,
    message: '等待扫描'
  }
}

describe('WeixinLoginDialog', () => {
  let harness: ReactHarness | null = null

  beforeEach(() => {
    installBrowserStubs()
    startMock.mockReset()
    waitMock.mockReset()
    waitMock.mockResolvedValue(pending())
  })

  afterEach(async () => {
    if (harness) {
      await harness.unmount()
      harness = null
    }
  })

  it('encodes a URL payload into a QR code instead of printing it', async () => {
    startMock.mockResolvedValue({
      session_id: 'sess-1',
      qr_content: 'https://ilinkai.weixin.qq.com/connect?token=abc',
      message: '请扫码'
    })
    harness = await mountWithProviders(<WeixinLoginDialog onClose={() => {}} />)
    await settle()

    const qr = harness.host.querySelector('.ark-im-qr svg')
    expect(qr, 'the login URL must be rendered as a QR code').not.toBeNull()
    expect(qr?.querySelector('path'), 'the QR must encode content, not be an empty shell').not.toBeNull()
    expect(harness.host.querySelector('.ark-im-qr img')).toBeNull()
    expect(harness.host.textContent).toContain('用手机微信扫描以下二维码')
  })

  it('renders an image payload as-is', async () => {
    startMock.mockResolvedValue({
      session_id: 'sess-2',
      qr_content: 'data:image/png;base64,aGVsbG8=',
      message: '请扫码'
    })
    harness = await mountWithProviders(<WeixinLoginDialog onClose={() => {}} />)
    await settle()

    const img = harness.host.querySelector('.ark-im-qr img')
    expect(img, 'a data:image payload is the picture itself').not.toBeNull()
    expect(img?.getAttribute('src')).toBe('data:image/png;base64,aGVsbG8=')
    expect(harness.host.querySelector('.ark-im-qr svg')).toBeNull()
  })

  it('still shows the skeleton while the session is being fetched', async () => {
    startMock.mockImplementation(() => new Promise(() => {}))
    harness = await mountWithProviders(<WeixinLoginDialog onClose={() => {}} />)
    await settle()
    expect(harness.host.querySelector('.ark-im-qr')).toBeNull()
    expect(harness.host.querySelector('.ark-skeleton'), 'loading state stays visible').not.toBeNull()
  })
})
