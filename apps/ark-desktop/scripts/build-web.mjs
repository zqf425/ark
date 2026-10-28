import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const webRoot = resolve(desktopRoot, '..', '..', 'web')
const npmCommand = process.platform === 'win32' ? (process.env.ComSpec ?? 'cmd.exe') : 'npm'
const npmArgs = process.platform === 'win32' ? ['/d', '/s', '/c', 'npm run build'] : ['run', 'build']
const result = spawnSync(npmCommand, npmArgs, {
  cwd: webRoot,
  env: { ...process.env, VITE_ARK_BASE_URL: 'http://127.0.0.1:18791' },
  stdio: 'inherit'
})

if (result.status !== 0) process.exit(result.status ?? 1)
