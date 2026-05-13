const { contextBridge, ipcRenderer } = require('electron')

const usesDevServer = process.argv.includes('--ingot-use-vite-dev-server=1')
const daemonOrigin = process.env.INGOT_API_ORIGIN ?? 'http://127.0.0.1:4190'
const desktopApi = {
  pickProjectDirectory: () => ipcRenderer.invoke('ingot:pick-project-directory'),
}

contextBridge.exposeInMainWorld(
  'ingotDesktop',
  usesDevServer
    ? desktopApi
    : {
        ...desktopApi,
        apiOrigin: 'ingot://app',
        wsOrigin: daemonOrigin,
      },
)
