const { spawn } = require('node:child_process')
const http = require('node:http')
const net = require('node:net')
const path = require('node:path')

const uiRoot = path.resolve(__dirname, '..')
const defaultViteUrl = 'http://127.0.0.1:4191'
const viteUrl = process.env.VITE_DEV_SERVER_URL ?? defaultViteUrl
const usesDefaultViteUrl = viteUrl === defaultViteUrl

const children = new Set()
let shuttingDown = false

function spawnChild(command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: uiRoot,
    env: process.env,
    stdio: 'inherit',
    ...options,
  })
  children.add(child)
  child.once('exit', () => {
    children.delete(child)
    if (!shuttingDown) {
      shutdown(child.exitCode ?? 1)
    }
  })
  return child
}

function waitForHttp(url, timeoutMs = 30_000) {
  const startedAt = Date.now()

  return new Promise((resolve, reject) => {
    const attempt = () => {
      const req = http.get(url, (res) => {
        res.resume()
        if (res.statusCode && res.statusCode >= 200 && res.statusCode < 500) {
          resolve()
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
        reject(new Error(`Timed out waiting for ${url}`))
        return
      }
      setTimeout(attempt, 250)
    }

    attempt()
  })
}

function assertPortAvailable(url) {
  const parsedUrl = new URL(url)
  const port = Number(parsedUrl.port || (parsedUrl.protocol === 'https:' ? 443 : 80))

  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: parsedUrl.hostname, port })

    socket.once('connect', () => {
      socket.destroy()
      reject(new Error(`Port ${port} is already in use at ${parsedUrl.origin}; electron:dev requires its own Vite server.`))
    })

    socket.once('error', (error) => {
      socket.destroy()
      if (error && error.code === 'ECONNREFUSED') {
        resolve()
        return
      }
      reject(error)
    })

    socket.setTimeout(1_000, () => {
      socket.destroy()
      reject(new Error(`Timed out checking whether ${parsedUrl.origin} is available`))
    })
  })
}

function shutdown(code = 0) {
  if (shuttingDown) return
  shuttingDown = true
  for (const child of children) {
    child.kill('SIGTERM')
  }
  setTimeout(() => process.exit(code), 100)
}

process.once('SIGINT', () => shutdown(130))
process.once('SIGTERM', () => shutdown(143))

async function main() {
  if (usesDefaultViteUrl) {
    await assertPortAvailable(viteUrl)
  }

  spawnChild('bun', ['run', 'dev', '--', '--host', '127.0.0.1', '--strictPort'])
  await waitForHttp(viteUrl)

  const electronBin = path.join(uiRoot, 'node_modules', '.bin', process.platform === 'win32' ? 'electron.cmd' : 'electron')
  spawnChild(electronBin, ['.'], {
    env: {
      ...process.env,
      VITE_DEV_SERVER_URL: viteUrl,
    },
  })
}

main().catch((error) => {
  console.error(error)
  shutdown(1)
})
