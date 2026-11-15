import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { resolveCargoTargetDir } from './prepare-sidecar.mjs'
import { DEV_PORTS, isDevStackProcess } from './dev.mjs'

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

test('desktop package routes dev through the lifetime-bound supervisor', () => {
  const packagePath = resolve(desktopRoot, 'package.json')
  assert.equal(existsSync(packagePath), true, 'desktop package must exist')

  const desktopPackage = readJson(packagePath)
  assert.equal(desktopPackage.scripts.dev, 'node scripts/dev.mjs')

  // The supervisor must still run tauri dev, not replace it.
  const supervisor = readFileSync(resolve(desktopRoot, 'scripts', 'dev.mjs'), 'utf8')
  assert.match(supervisor, /spawn\(tauriBin, \['dev'\]/)
})

test('dev supervisor sweeps exactly the ports the dev stack owns', () => {
  const config = readJson(resolve(desktopRoot, 'src-tauri', 'tauri.conf.json'))
  const devPort = new URL(config.build.devUrl).port
  assert.deepEqual([...DEV_PORTS].sort(), ['18791', devPort].sort())
})

test('dev supervisor only kills processes that belong to the ark dev stack', () => {
  const ours = [
    'node /repo/web/node_modules/.bin/vite',
    'target/debug/arkd serve --port 18791',
    'cargo run --package arkd -- serve',
    'node scripts/dev-web.mjs',
    'target/debug/ark'
  ]
  for (const command of ours) {
    assert.equal(isDevStackProcess(command), true, `must match: ${command}`)
  }

  const foreign = [
    'redis-server *:5273',
    'python3 -m http.server 18791',
    '/usr/sbin/nginx -g daemon off;'
  ]
  for (const command of foreign) {
    assert.equal(isDevStackProcess(command), false, `must not match: ${command}`)
  }
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
  // Since 601334d the window is created by the Rust host after arkd answers
  // `/health` (runtime.rs), so the config must not declare one; the label
  // must stay "main" so the `default` capability keeps applying.
  assert.deepEqual(config.app.windows, [])
  const runtimeRs = readFileSync(resolve(desktopRoot, 'src-tauri', 'src', 'runtime.rs'), 'utf8')
  assert.match(runtimeRs, /WebviewWindowBuilder::new\(app, "main"/)
  assert.match(runtimeRs, /\.title\("ark"\)/)
  assert.match(config.app.security.csp, /connect-src[^;]*http:\/\/127\.0\.0\.1:18791/)
  assert.equal(config.bundle.externalBin, undefined, 'development must not require a release sidecar artifact')

  const viteConfig = readFileSync(resolve(repositoryRoot, 'web', 'vite.config.ts'), 'utf8')
  assert.match(viteConfig, /host:\s*'127\.0\.0\.1'/)
})

test('the shell exposes system notifications for turn ends', () => {
  const mainRs = readFileSync(resolve(desktopRoot, 'src-tauri', 'src', 'main.rs'), 'utf8')
  assert.match(mainRs, /tauri_plugin_notification::init\(\)/, 'notification plugin must be registered')
  assert.match(
    mainRs,
    /fn notify_session_event\(app: tauri::AppHandle, title: String, body: String\)/,
    'system notification command must take display text only'
  )
  assert.match(
    mainRs,
    /notify_session_event/,
    'the command must be reachable from the renderer invoke handler'
  )

  const desktopBridge = readFileSync(resolve(repositoryRoot, 'web', 'src', 'shared', 'desktop.ts'), 'utf8')
  assert.match(desktopBridge, /notify_session_event/, 'the web bridge must call the shell command')
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
