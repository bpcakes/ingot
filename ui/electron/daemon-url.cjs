const DEFAULT_API_ORIGIN = 'http://127.0.0.1:4190'

function normalizeApiOrigin(value) {
  const rawValue = value === undefined ? DEFAULT_API_ORIGIN : value

  if (typeof rawValue !== 'string' || rawValue.trim() === '') {
    throw new Error(`INGOT_API_ORIGIN must be an HTTP(S) origin, got ${JSON.stringify(rawValue)}`)
  }

  let url
  try {
    url = new URL(rawValue.trim())
  } catch (error) {
    throw new Error(`INGOT_API_ORIGIN must be a valid HTTP(S) origin, got ${JSON.stringify(rawValue)}`)
  }

  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`INGOT_API_ORIGIN must use http or https, got ${JSON.stringify(rawValue)}`)
  }

  if (url.username !== '' || url.password !== '') {
    throw new Error(`INGOT_API_ORIGIN must not include credentials, got ${JSON.stringify(rawValue)}`)
  }

  if (!/^\/*$/.test(url.pathname) || url.search !== '' || url.hash !== '') {
    throw new Error(`INGOT_API_ORIGIN must not include a path, query, or fragment, got ${JSON.stringify(rawValue)}`)
  }

  return url.origin
}

function daemonApiUrl(apiOrigin, pathname, search = '') {
  if (typeof pathname !== 'string' || (pathname !== '/api' && !pathname.startsWith('/api/'))) {
    throw new Error(`Daemon API path must start with /api, got ${JSON.stringify(pathname)}`)
  }

  if (search !== undefined && search !== '' && (typeof search !== 'string' || !search.startsWith('?'))) {
    throw new Error(`Daemon API search must be empty or start with ?, got ${JSON.stringify(search)}`)
  }

  const url = new URL(pathname, `${normalizeApiOrigin(apiOrigin)}/`)
  url.search = search ?? ''
  return url.toString()
}

module.exports = {
  DEFAULT_API_ORIGIN,
  daemonApiUrl,
  normalizeApiOrigin,
}
