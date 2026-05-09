import { getApiBaseUrl, getWebSocketUrl } from '../api/base'

describe('api base helpers', () => {
  afterEach(() => {
    delete window.ingotDesktop
    vi.unstubAllEnvs()
  })

  it('uses same-origin paths in the browser by default', () => {
    expect(getApiBaseUrl()).toBe('/api')
    expect(getWebSocketUrl()).toBe('ws://localhost:3000/api/ws')
  })

  it('uses the Electron preload origin when present', () => {
    window.ingotDesktop = {
      apiOrigin: 'ingot://app/',
      wsOrigin: 'http://127.0.0.1:4190/',
    }

    expect(getApiBaseUrl()).toBe('ingot://app/api')
    expect(getWebSocketUrl()).toBe('ws://127.0.0.1:4190/api/ws')
  })

  it('uses the Vite API origin environment override outside Electron', () => {
    vi.stubEnv('VITE_INGOT_API_ORIGIN', 'https://example.test')

    expect(getApiBaseUrl()).toBe('https://example.test/api')
    expect(getWebSocketUrl()).toBe('wss://example.test/api/ws')
  })
})
