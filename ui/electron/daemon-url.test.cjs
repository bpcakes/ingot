const assert = require('node:assert/strict')
const test = require('node:test')

const { DEFAULT_API_ORIGIN, daemonApiUrl, normalizeApiOrigin } = require('./daemon-url.cjs')

test('normalizes the default daemon origin', () => {
  assert.equal(normalizeApiOrigin(undefined), DEFAULT_API_ORIGIN)
})

test('normalizes a daemon origin with a trailing slash', () => {
  assert.equal(normalizeApiOrigin('http://127.0.0.1:4190/'), DEFAULT_API_ORIGIN)
  assert.equal(normalizeApiOrigin('http://127.0.0.1:4190///'), DEFAULT_API_ORIGIN)
})

test('constructs the daemon health URL', () => {
  assert.equal(daemonApiUrl(undefined, '/api/health'), 'http://127.0.0.1:4190/api/health')
})

test('constructs proxied API URLs without duplicate slashes', () => {
  assert.equal(
    daemonApiUrl('http://127.0.0.1:4190/', '/api/projects', '?limit=1'),
    'http://127.0.0.1:4190/api/projects?limit=1',
  )
})

test('rejects invalid daemon origins with a clear environment variable name', () => {
  assert.throws(() => normalizeApiOrigin('not-a-url'), /INGOT_API_ORIGIN/)
})

test('rejects unsupported daemon origin protocols', () => {
  assert.throws(() => normalizeApiOrigin('ws://127.0.0.1:4190'), /http or https/)
})

test('rejects daemon origins with paths', () => {
  assert.throws(() => normalizeApiOrigin('http://127.0.0.1:4190/base'), /must not include a path/)
})

test('rejects proxy paths outside the daemon API', () => {
  assert.throws(() => daemonApiUrl(undefined, '/assets/app.js'), /must start with \/api/)
})
