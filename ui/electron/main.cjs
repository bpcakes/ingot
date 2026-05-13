const { app, BrowserWindow, dialog, ipcMain, protocol } = require('electron')
const { spawn } = require('node:child_process')
const fs = require('node:fs')
const http = require('node:http')
const https = require('node:https')
const path = require('node:path')
const { daemonApiUrl, normalizeApiOrigin } = require('./daemon-url.cjs')
const {
  rendererEventIsTrusted,
  rendererUrlIsTrusted: rendererUrlMatchesTrustedOrigins,
} = require('./desktop-security.cjs')

const isDev = !app.isPackaged
const usesDevServer = isDev && Boolean(process.env.VITE_DEV_SERVER_URL)
const devServerUrl = usesDevServer ? process.env.VITE_DEV_SERVER_URL : undefined

let apiOrigin = null
let daemonProcess = null
let mainWindow = null

protocol.registerSchemesAsPrivileged([
  {
    scheme: 'ingot',
    privileges: {
      standard: true,
      secure: true,
      supportFetchAPI: true,
      corsEnabled: true,
    },
  },
])

function repoRoot() {
  return path.resolve(__dirname, '..', '..')
}

function packagedDaemonPath() {
  const binaryName = process.platform === 'win32' ? 'ingotd.exe' : 'ingotd'
  return path.join(process.resourcesPath, 'bin', binaryName)
}

function daemonCommand() {
  if (isDev) {
    return {
      command: 'cargo',
      args: ['run', '--bin', 'ingotd'],
      cwd: repoRoot(),
    }
  }

  return {
    command: packagedDaemonPath(),
    args: [],
    cwd: path.dirname(process.resourcesPath),
  }
}

function getApiOrigin() {
  if (apiOrigin === null) {
    apiOrigin = normalizeApiOrigin(process.env.INGOT_API_ORIGIN)
  }
  return apiOrigin
}

function rendererUrlIsTrusted(rawUrl) {
  return rendererUrlMatchesTrustedOrigins(rawUrl, {
    isDev: usesDevServer,
    devServerUrl,
  })
}

function ensureTrustedIpcSender(event) {
  if (
    !rendererEventIsTrusted(event, {
      isDev: usesDevServer,
      devServerUrl,
    })
  ) {
    throw new Error('Rejected IPC request from untrusted renderer')
  }
}

function healthUrl() {
  return daemonApiUrl(getApiOrigin(), '/api/health')
}

function get(url, responseHandler) {
  const client = new URL(url).protocol === 'https:' ? https : http
  return client.get(url, responseHandler)
}

function waitForHealth(timeoutMs = 30_000) {
  const url = healthUrl()
  const startedAt = Date.now()

  return new Promise((resolve, reject) => {
    const attempt = () => {
      const req = get(url, (res) => {
        res.resume()
        if (res.statusCode === 200) {
          resolve(true)
          return
        }
        retry()
      })

      req.once('error', retry)
      req.setTimeout(1_000, () => {
        req.destroy()
        retry()
      })
    }

    const retry = () => {
      if (Date.now() - startedAt > timeoutMs) {
        reject(new Error(`Timed out waiting for ingotd at ${url}`))
        return
      }
      setTimeout(attempt, 250)
    }

    attempt()
  })
}

function checkHealthOnce() {
  const url = healthUrl()

  return new Promise((resolve) => {
    const req = get(url, (res) => {
      res.resume()
      resolve(res.statusCode === 200)
    })

    req.once('error', () => resolve(false))
    req.setTimeout(1_000, () => {
      req.destroy()
      resolve(false)
    })
  })
}

function daemonExitMessage(code, signal) {
  if (signal) {
    return `ingotd exited before it became healthy (signal ${signal})`
  }
  return `ingotd exited before it became healthy (code ${code ?? 'unknown'})`
}

async function ensureDaemon() {
  if (await checkHealthOnce()) return

  const command = daemonCommand()
  let child
  try {
    child = spawn(command.command, command.args, {
      cwd: command.cwd,
      env: process.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new Error(`Unable to launch ingotd: ${message}`)
  }
  daemonProcess = child

  child.stdout.on('data', (chunk) => {
    process.stdout.write(`[ingotd] ${chunk}`)
  })
  child.stderr.on('data', (chunk) => {
    process.stderr.write(`[ingotd] ${chunk}`)
  })

  let healthReady = false
  const startupFailure = new Promise((_, reject) => {
    child.once('error', (error) => {
      if (daemonProcess === child) {
        daemonProcess = null
      }
      reject(new Error(`Unable to launch ingotd: ${error.message}`))
    })

    child.once('exit', (code, signal) => {
      if (daemonProcess === child) {
        daemonProcess = null
      }
      if (!healthReady) {
        reject(new Error(daemonExitMessage(code, signal)))
      }
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('ingot-daemon-exit', { code, signal })
      }
    })
  })

  await Promise.race([
    waitForHealth().then(() => {
      healthReady = true
    }),
    startupFailure,
  ])
}

function clearDaemonProcess() {
  if (daemonProcess) {
    daemonProcess.kill('SIGTERM')
    daemonProcess = null
  }
}

function distRoot() {
  return path.resolve(__dirname, '..', 'dist')
}

function contentType(filePath) {
  switch (path.extname(filePath)) {
    case '.html':
      return 'text/html'
    case '.js':
      return 'text/javascript'
    case '.css':
      return 'text/css'
    case '.svg':
      return 'image/svg+xml'
    case '.png':
      return 'image/png'
    case '.jpg':
    case '.jpeg':
      return 'image/jpeg'
    case '.woff2':
      return 'font/woff2'
    default:
      return 'application/octet-stream'
  }
}

function fileResponse(filePath) {
  return new Response(fs.readFileSync(filePath), {
    headers: {
      'content-type': contentType(filePath),
    },
  })
}

function registerAppProtocol() {
  protocol.handle('ingot', async (request) => {
    const url = new URL(request.url)
    if (url.pathname === '/api' || url.pathname.startsWith('/api/')) {
      return proxyApi(request, url)
    }

    const root = distRoot()
    const requestedPath = decodeURIComponent(url.pathname === '/' ? '/index.html' : url.pathname)
    const resolvedPath = path.resolve(root, `.${requestedPath}`)
    const relativePath = path.relative(root, resolvedPath)

    if (relativePath.startsWith('..') || path.isAbsolute(relativePath)) {
      return new Response('Not found', { status: 404 })
    }

    if (fs.existsSync(resolvedPath) && fs.statSync(resolvedPath).isFile()) {
      return fileResponse(resolvedPath)
    }

    return fileResponse(path.join(root, 'index.html'))
  })
}

function registerDesktopIpc() {
  ipcMain.handle('ingot:pick-project-directory', async (event) => {
    ensureTrustedIpcSender(event)

    const owner = BrowserWindow.fromWebContents(event.sender)
    if (!owner || owner.isDestroyed()) {
      const error = new Error('Unable to resolve project picker window')
      console.error(error)
      throw error
    }

    const options = {
      title: 'Select project repository',
      buttonLabel: 'Select repository',
      properties: ['openDirectory'],
    }
    const result = await dialog.showOpenDialog(owner, options)

    if (result.canceled || result.filePaths.length === 0) {
      return null
    }

    return result.filePaths[0]
  })
}

function limitWindowNavigation(win) {
  win.webContents.on('will-navigate', (event, url) => {
    if (!rendererUrlIsTrusted(url)) {
      console.warn(`Blocked untrusted navigation to ${url}`)
      event.preventDefault()
    }
  })

  win.webContents.on('will-redirect', (event, url) => {
    if (!rendererUrlIsTrusted(url)) {
      console.warn(`Blocked untrusted redirect to ${url}`)
      event.preventDefault()
    }
  })

  // Links can originate from project-authored markdown, so keep new windows disabled until an allowlist exists.
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
}

function createTrustedBrowserWindow(options) {
  const win = new BrowserWindow(options)
  limitWindowNavigation(win)
  return win
}

async function proxyApi(request, url) {
  const targetUrl = daemonApiUrl(getApiOrigin(), url.pathname, url.search)
  const method = request.method.toUpperCase()
  const headers = new Headers(request.headers)
  headers.delete('host')
  headers.delete('origin')

  const response = await fetch(targetUrl, {
    method,
    headers,
    body: method === 'GET' || method === 'HEAD' ? undefined : Buffer.from(await request.arrayBuffer()),
    redirect: 'manual',
  })

  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  })
}

async function createWindow() {
  mainWindow = createTrustedBrowserWindow({
    width: 1280,
    height: 860,
    minWidth: 980,
    minHeight: 640,
    title: 'Ingot',
    webPreferences: {
      // Preload uses this argument to choose the same dev/prod bridge shape as the main process.
      additionalArguments: [`--ingot-use-vite-dev-server=${usesDevServer ? '1' : '0'}`],
      contextIsolation: true,
      nodeIntegration: false,
      preload: path.join(__dirname, 'preload.cjs'),
    },
  })

  if (usesDevServer) {
    await mainWindow.loadURL(devServerUrl)
    mainWindow.webContents.openDevTools({ mode: 'detach' })
    return
  }

  await mainWindow.loadURL('ingot://app/')
}

app.whenReady().then(async () => {
  registerAppProtocol()
  registerDesktopIpc()

  try {
    await ensureDaemon()
    await createWindow()
  } catch (error) {
    dialog.showErrorBox('Unable to start Ingot', error instanceof Error ? error.message : String(error))
    app.quit()
  }

  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow().catch((error) => {
        dialog.showErrorBox('Unable to open Ingot', error instanceof Error ? error.message : String(error))
      })
    }
  })
})

app.on('before-quit', clearDaemonProcess)

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit()
  }
})
