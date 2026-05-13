const assert = require('node:assert/strict')
const test = require('node:test')

const { rendererEventIsTrusted, rendererUrlIsTrusted } = require('./desktop-security.cjs')

test('trusts the packaged ingot app origin', () => {
  assert.equal(rendererUrlIsTrusted('ingot://app/'), true)
  assert.equal(rendererUrlIsTrusted('ingot://app/projects/prj_1'), true)
})

test('trusts the configured dev server origin only in development', () => {
  const options = { isDev: true, devServerUrl: 'http://127.0.0.1:5173/' }

  assert.equal(rendererUrlIsTrusted('http://127.0.0.1:5173/projects', options), true)
  assert.equal(rendererUrlIsTrusted('http://localhost:5173/projects', options), false)
  assert.equal(rendererUrlIsTrusted('http://127.0.0.1:5174/projects', options), false)
  assert.equal(rendererUrlIsTrusted('http://127.0.0.1:5173/projects', { ...options, isDev: false }), false)
})

test('rejects invalid and untrusted renderer URLs', () => {
  assert.equal(rendererUrlIsTrusted(undefined), false)
  assert.equal(rendererUrlIsTrusted(''), false)
  assert.equal(rendererUrlIsTrusted('not-a-url'), false)
  assert.equal(rendererUrlIsTrusted('https://example.com'), false)
  assert.equal(rendererUrlIsTrusted('ingot://evil/'), false)
})

test('trusts renderer events by sender frame URL', () => {
  assert.equal(rendererEventIsTrusted({ senderFrame: { url: 'ingot://app/' } }), true)
  assert.equal(rendererEventIsTrusted({ senderFrame: { url: 'https://example.com/' } }), false)
  assert.equal(rendererEventIsTrusted({}), false)
})

test('falls back to the web contents URL only when sender frame URL is unavailable', () => {
  assert.equal(rendererEventIsTrusted({ sender: { getURL: () => 'ingot://app/' } }), true)
  assert.equal(
    rendererEventIsTrusted({
      senderFrame: { url: 'https://example.com/' },
      sender: { getURL: () => 'ingot://app/' },
    }),
    false,
  )
})
