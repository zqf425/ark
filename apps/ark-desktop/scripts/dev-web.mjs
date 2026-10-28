import { spawn } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const webRoot = resolve(desktopRoot, '..', '..', 'web')
const npmCommand = process.platform === 'win32' ? (process.env.ComSpec ?? 'cmd.exe') : 'npm'
const npmArgs = process.platform === 'win32' ? ['/d', '/s', '/c', 'npm run dev'] : ['run', 'dev']
const child = spawn(npmCommand, npmArgs, {
  cwd: webRoot,
  env: { ...process.env, ARK_RUNTIME_PORT: '18791' },
  stdio: 'inherit'
})

child.once('exit', (code) => process.exit(code ?? 1))
child.once('error', (error) => {
  console.error(`无法启动 Web 开发服务器：${error.message}`)
  process.exit(1)
})
