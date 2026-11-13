// Dev supervisor for `npm run dev`.
//
// It owns the lifetime binding between the npm command and the whole dev stack:
//
//   npm run dev → 本脚本 → tauri dev → vite (:5273) + ark 桌面 app → arkd (:18791)
//
// Two guarantees:
// 1. Ctrl+C / killing the npm command tears down tauri's whole process group,
//    then sweeps any leftover dev-stack listener on the dev ports. Nothing
//    keeps listening after the command dies — npm layers and IDE terminals
//    swallow or misroute signals, so relying on tauri's own cleanup is not
//    enough (see the `wait: false` beforeDevCommand in tauri.conf.json).
// 2. Quitting the desktop app ends the npm command: tauri dev exits with the
//    app, this supervisor sweeps leftovers (e.g. an arkd that outlived a
//    killed app) and exits, which ends the npm chain the same way.
//
// The port sweep only ever kills processes whose command line looks like our
// own dev stack; an unrelated service that owns one of the ports is reported,
// never killed.

import { spawn, execFile } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')

// Keep in sync with: vite dev server (tauri.conf.json `build.devUrl`) and the
// local arkd runtime (src-tauri/src/runtime.rs `RUNTIME_ADDRESS`).
export const DEV_PORTS = ['5273', '18791']

const TEARDOWN_GRACE_MS = 1500
const SWEEP_GRACE_MS = 400

export function isDevStackProcess(commandLine) {
  return /vite|arkd|tauri|cargo|dev-web|ark-desktop|target[\\/]debug[\\/]ark/i.test(commandLine)
}

const delay = (ms) => new Promise((done) => setTimeout(done, ms))

function listListenerPids() {
  return new Promise((resolvePids) => {
    execFile(
      'lsof',
      ['-t', '-nP', ...DEV_PORTS.map((port) => `-iTCP:${port}`), '-sTCP:LISTEN'],
      (_error, stdout) => {
        // lsof exits non-zero when nothing matches; an empty result is fine.
        const pids = stdout
          .split('\n')
          .map((line) => Number.parseInt(line.trim(), 10))
          .filter(Number.isInteger)
        resolvePids(pids)
      }
    )
  })
}

function commandOf(pid) {
  return new Promise((resolveCommand) => {
    execFile('ps', ['-o', 'command=', '-p', String(pid)], (_error, stdout) => {
      resolveCommand(stdout.trim())
    })
  })
}

async function sweepListeners(signal) {
  for (const pid of await listListenerPids()) {
    const command = await commandOf(pid)
    if (!isDevStackProcess(command)) {
      console.warn(`[dev] 端口被非开发栈进程占用，跳过：pid=${pid} ${command}`)
      continue
    }
    try {
      process.kill(pid, signal)
    } catch {
      // Already gone.
    }
  }
}

async function main() {
  const tauriBin = join(desktopRoot, 'node_modules', '.bin', 'tauri')

  let child = null
  let shuttingDown = false

  const killGroup = (signal) => {
    if (!child?.pid) return
    if (process.platform === 'win32') {
      spawn('taskkill', ['/PID', String(child.pid), '/T', '/F'])
      return
    }
    try {
      // Negative pid targets the whole process group: tauri dev, cargo, the
      // app binary and the vite chain all share it (spawned detached).
      process.kill(-child.pid, signal)
    } catch {
      // The group is already gone — nothing left to stop.
    }
  }

  const shutdown = async (exitCode) => {
    if (shuttingDown) return
    shuttingDown = true
    killGroup('SIGTERM')
    await delay(TEARDOWN_GRACE_MS)
    killGroup('SIGKILL')
    await sweepListeners('SIGTERM')
    await delay(SWEEP_GRACE_MS)
    await sweepListeners('SIGKILL')
    process.exit(exitCode)
  }

  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    process.on(signal, () => {
      void shutdown(signal === 'SIGINT' ? 130 : 143)
    })
  }

  child = spawn(tauriBin, ['dev'], {
    cwd: desktopRoot,
    stdio: 'inherit',
    // Own process group: the supervisor — not the terminal — decides when the
    // whole subtree dies, and one signal reaches every descendant.
    detached: process.platform !== 'win32'
  })

  child.on('exit', (code) => {
    // The desktop app quit (or tauri died): end the npm command and take any
    // surviving dev-stack listener with us. Group kill still applies — the
    // group id survives its dead leader as long as members remain.
    void shutdown(code ?? 1)
  })
  child.on('error', (error) => {
    console.error(`[dev] 无法启动 tauri dev：${error.message}`)
    process.exit(1)
  })
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => {
    console.error(`[dev] ${error?.message ?? error}`)
    process.exit(1)
  })
}
