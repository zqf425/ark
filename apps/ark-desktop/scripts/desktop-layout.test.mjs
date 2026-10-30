import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { resolveCargoTargetDir } from './prepare-sidecar.mjs'

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repositoryRoot = resolve(desktopRoot, '..', '..')

function readJson(path) {
  return JSON.parse(readFileSync(path, 'utf8'))
}

test('root dev command launches the native desktop shell', () => {
  const packagePath = resolve(repositoryRoot, 'package.json')
  assert.equal(existsSync(packagePath), true, 'root package.json must expose the desktop entrypoint')

  const rootPackage = readJson(packagePath)
  assert.equal(rootPackage.scripts.dev, 'npm --prefix apps/ark-desktop run dev')
})

test('desktop package uses Tauri for development', () => {
  const packagePath = resolve(desktopRoot, 'package.json')
  assert.equal(existsSync(packagePath), true, 'desktop package must exist')

  const desktopPackage = readJson(packagePath)
  assert.equal(desktopPackage.scripts.dev, 'tauri dev')
})

test('Tauri configuration owns the window and delegates web development to Vite', () => {
  const configPath = resolve(desktopRoot, 'src-tauri', 'tauri.conf.json')
  assert.equal(existsSync(configPath), true, 'Tauri configuration must exist')

  const config = readJson(configPath)
  assert.equal(config.build.devUrl, 'http://127.0.0.1:5273')
  assert.deepEqual(config.build.beforeDevCommand, {
    script: 'npm run dev:web',
    cwd: '..',
    wait: false
  })
  assert.equal(config.build.frontendDist, '../../../web/dist')
  assert.equal(config.app.windows[0].label, 'main')
  assert.equal(config.app.windows[0].title, 'ark')
  assert.match(config.app.security.csp, /connect-src[^;]*http:\/\/127\.0\.0\.1:18791/)
  assert.equal(config.bundle.externalBin, undefined, 'development must not require a release sidecar artifact')

  const viteConfig = readFileSync(resolve(repositoryRoot, 'web', 'vite.config.ts'), 'utf8')
  assert.match(viteConfig, /host:\s*'127\.0\.0\.1'/)
})

test('release configuration is the only place that embeds the arkd sidecar', () => {
  const configPath = resolve(desktopRoot, 'src-tauri', 'tauri.release.conf.json')
  assert.equal(existsSync(configPath), true, 'release sidecar configuration must exist')

  const config = readJson(configPath)
  assert.deepEqual(config.bundle.externalBin, ['binaries/arkd'])

  const desktopPackage = readJson(resolve(desktopRoot, 'package.json'))
  assert.equal(desktopPackage.scripts.build, 'tauri build --config src-tauri/tauri.release.conf.json')
})

test('sidecar preparation resolves default and isolated Cargo target directories', () => {
  assert.equal(resolveCargoTargetDir(repositoryRoot, undefined), resolve(repositoryRoot, 'target'))
  assert.equal(resolveCargoTargetDir(repositoryRoot, '   '), resolve(repositoryRoot, 'target'))
  assert.equal(
    resolveCargoTargetDir(repositoryRoot, 'artifacts/cargo-target'),
    resolve(repositoryRoot, 'artifacts/cargo-target')
  )

  const absoluteTargetDir = resolve(repositoryRoot, '..', 'isolated-cargo-target')
  assert.equal(resolveCargoTargetDir(repositoryRoot, absoluteTargetDir), absoluteTargetDir)
})
