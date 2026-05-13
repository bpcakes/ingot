function rendererUrlIsTrusted(rawUrl, options = {}) {
  const { isDev = false, devServerUrl } = options

  if (typeof rawUrl !== 'string' || rawUrl.trim() === '') {
    return false
  }

  let url
  try {
    url = new URL(rawUrl)
  } catch {
    return false
  }

  if (url.protocol === 'ingot:' && url.hostname === 'app') {
    return true
  }

  if (!isDev || !devServerUrl) {
    return false
  }

  try {
    // Keep this strict: the trusted dev origin must match the URL Electron actually loaded.
    return url.origin === new URL(devServerUrl).origin
  } catch {
    return false
  }
}

function rendererEventIsTrusted(event, options = {}) {
  // Trust the frame that invoked IPC; fall back to the top-level URL only if Electron cannot provide it.
  const senderFrameUrl = event?.senderFrame?.url
  if (typeof senderFrameUrl === 'string' && senderFrameUrl !== '') {
    return rendererUrlIsTrusted(senderFrameUrl, options)
  }

  return rendererUrlIsTrusted(event?.sender?.getURL?.(), options)
}

module.exports = {
  rendererEventIsTrusted,
  rendererUrlIsTrusted,
}
