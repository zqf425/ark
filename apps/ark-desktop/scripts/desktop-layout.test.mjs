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

  // The supervisor must still run tauri dev, not replace it. Windows launches
  // the CLI entry through node because the extensionless npm shim is not
  // executable by CreateProcess; Unix keeps using the shim.
  const supervisor = readFileSync(resolve(desktopRoot, 'scripts', 'dev.mjs'), 'utf8')
  assert.match(supervisor, /process\.platform === 'win32'/)
  assert.match(supervisor, /'@tauri-apps', 'cli', 'tauri\.js'/)
  assert.match(supervisor, /spawn\(tauriLaunch\.command, \[\.\.\.tauriLaunch\.args, 'dev'\]/)
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
  // HTML5 drag-and-drop (composer file attachments) needs the native drop
  // handler disabled; on Windows the default handler swallows page drop events.
  assert.match(runtimeRs, /\.disable_drag_drop_handler\(\)/)
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

test('external links use the opener plugin instead of the deprecated shell API', () => {
  const manifest = readFileSync(resolve(desktopRoot, 'src-tauri', 'Cargo.toml'), 'utf8')
  const mainRs = readFileSync(resolve(desktopRoot, 'src-tauri', 'src', 'main.rs'), 'utf8')
  const runtimeRs = readFileSync(resolve(desktopRoot, 'src-tauri', 'src', 'runtime.rs'), 'utf8')

  assert.match(manifest, /^tauri-plugin-opener = "2"$/m)
  assert.match(mainRs, /\.plugin\(tauri_plugin_opener::init\(\)\)/)
  assert.match(mainRs, /app\.opener\(\)\s*\.open_url\(parsed\.as_str\(\), None::<&str>\)/)
  assert.match(runtimeRs, /app\.opener\(\)\s*\.open_url\(url\.as_str\(\), None::<&str>\)/)
  assert.doesNotMatch(mainRs, /\.shell\(\)\s*\.open\(/)
  assert.doesNotMatch(runtimeRs, /\.shell\(\)\s*\.open\(/)
})

test('the folder picker is owned by the main window', () => {
  const mainRs = readFileSync(resolve(desktopRoot, 'src-tauri', 'src', 'main.rs'), 'utf8')

  assert.match(
    mainRs,
    /async fn pick_folder\([^)]*window: tauri::Window[^)]*\)/,
    'the command must receive the calling window from Tauri'
  )
  assert.match(
    mainRs,
    /\.file\(\)[\s\S]*?\.set_parent\(&window\)[\s\S]*?\.pick_folder\(/,
    'the dialog must set the calling window as its parent so Windows brings it forward'
  )
})

test('release configuration is the only place that embeds the arkd sidecar', () => {
  const configPath = resolve(desktopRoot, 'src-tauri', 'tauri.release.conf.json')
  assert.equal(existsSync(configPath), true, 'release sidecar configuration must exist')

  const config = readJson(configPath)
  assert.deepEqual(config.bundle.externalBin, ['binaries/arkd'])

  const desktopPackage = readJson(resolve(desktopRoot, 'package.json'))
  assert.equal(desktopPackage.scripts.build, 'tauri build --config src-tauri/tauri.release.conf.json')
})

test('managed runtime keeps process-group teardown explicit', () => {
  const runtimeRs = readFileSync(resolve(desktopRoot, 'src-tauri', 'src', 'runtime.rs'), 'utf8')
  assert.equal(runtimeRs.match(/\.set_process_group\(true\)/g)?.length, 2)
  assert.equal(runtimeRs.match(/\.kill_on_drop\(false\)/g)?.length, 2)
})

test('NSIS stops only the sidecar from the current installation', () => {
  const hooks = readFileSync(
    resolve(desktopRoot, 'src-tauri', 'nsis', 'installer-hooks.nsh'),
    'utf8'
  )

  assert.doesNotMatch(hooks, /taskkill(?:\.exe)?["'][^\r\n]*\/IM\s+arkd\.exe/i)
  assert.match(hooks, /InitPluginsDir/, 'the installer must initialize the temporary script directory')
  assert.match(hooks, /CreateDirectory "\$PLUGINSDIR"/, 'the temporary script directory must exist')
  assert.match(hooks, /Where-Object\s+\{\s*\$\$?_\.Path\s+-eq\s+\$\$?ExecutablePath\s+\}/)
  assert.match(hooks, /-File\s+"\$PLUGINSDIR\\ark-stop-sidecar\.ps1"\s+"\$INSTDIR\\arkd\.exe"/)
  assert.match(hooks, /\$\{If\}\s+\$0\s+!=\s+0/, 'nonzero exit codes must be handled')
})

test('core verification runs strict Clippy after the workspace check', () => {
  const verify = readFileSync(resolve(repositoryRoot, 'scripts', 'verify.ps1'), 'utf8')
  const core = verify.match(/function Invoke-CoreChecks \{([\s\S]*?)\n\}/)?.[1]

  assert.ok(core, 'core scope must exist')
  assert.match(core, /Invoke-CheckedNative cargo @\('check', '--workspace'\)/)
  assert.match(core, /Invoke-CheckedNative cargo @\('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'\)/)
})

test('web verification includes an explicit typecheck', () => {
  const verify = readFileSync(resolve(repositoryRoot, 'scripts', 'verify.ps1'), 'utf8')
  const web = verify.match(/function Invoke-WebChecks \{([\s\S]*?)\n\}/)?.[1]

  assert.ok(web, 'web scope must exist')
  assert.match(web, /Invoke-CheckedNative npm @\('--prefix', 'web', 'run', 'typecheck'\)/)
})

test('desktop verification checks the Rust host in the existing CI job', () => {
  const verify = readFileSync(resolve(repositoryRoot, 'scripts', 'verify.ps1'), 'utf8')
  const desktop = verify.match(/function Invoke-DesktopChecks \{([\s\S]*?)\n\}/)?.[1]
  const workflow = readFileSync(resolve(repositoryRoot, '.github', 'workflows', 'verify.yml'), 'utf8')

  assert.ok(desktop, 'desktop scope must exist')
  assert.match(desktop, /Enable-GnuToolchain/)
  assert.match(desktop, /Invoke-CheckedNative cargo @\('check', '-p', 'ark-desktop'\)/)
  assert.match(workflow, /desktop:[\s\S]*?dtolnay\/rust-toolchain@stable/)
  assert.match(workflow, /run: \.\/scripts\/verify\.ps1 -Scope desktop/)
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
