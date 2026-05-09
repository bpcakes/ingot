const API_PATH = '/api'

export function getApiBaseUrl(): string {
  const origin = desktopApiOrigin() ?? envApiOrigin()
  if (!origin) return API_PATH

  return `${trimTrailingSlash(origin)}${API_PATH}`
}

export function getWebSocketUrl(): string {
  const origin = desktopWebSocketOrigin() ?? envApiOrigin()
  if (origin) {
    const url = new URL('/api/ws', origin)
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    return url.toString()
  }

  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${protocol}//${location.host}/api/ws`
}

function desktopApiOrigin(): string | undefined {
  return window.ingotDesktop?.apiOrigin
}

function desktopWebSocketOrigin(): string | undefined {
  return window.ingotDesktop?.wsOrigin
}

function envApiOrigin(): string | undefined {
  return import.meta.env.VITE_INGOT_API_ORIGIN
}

function trimTrailingSlash(value: string): string {
  return value.replace(/\/+$/, '')
}
