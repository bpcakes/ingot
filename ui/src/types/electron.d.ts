export {}

declare global {
  interface Window {
    ingotDesktop?: {
      apiOrigin?: string
      wsOrigin?: string
      pickProjectDirectory?: () => Promise<string | null>
    }
  }
}
