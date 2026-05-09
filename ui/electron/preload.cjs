const { contextBridge } = require('electron')

const isDev = Boolean(process.env.VITE_DEV_SERVER_URL)
const daemonOrigin = process.env.INGOT_API_ORIGIN ?? 'http://127.0.0.1:4190'

contextBridge.exposeInMainWorld(
  'ingotDesktop',
  isDev
    ? {}
    : {
        apiOrigin: 'ingot://app',
        wsOrigin: daemonOrigin,
      },
)
