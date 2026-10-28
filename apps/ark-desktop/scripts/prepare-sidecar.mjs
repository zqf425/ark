import { execFileSync, spawnSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repositoryRoot = resolve(desktopRoot, '..', '..')
const extension = process.platform === 'win32' ? '.exe' : ''
const targetTriple = execFileSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' }).trim()
const source = resolve(repositoryRoot, 'target', 'release', `arkd${extension}`)
const destination = resolve(desktopRoot, 'src-tauri', 'binaries', `arkd-${targetTriple}${extension}`)
const cargoCommand = process.platform === 'win32' ? 'cargo.exe' : 'cargo'

const build = spawnSync(cargoCommand, ['build', '--release', '--package', 'arkd'], {
  cwd: repositoryRoot,
  stdio: 'inherit'
})
if (build.status !== 0) process.exit(build.status ?? 1)
if (!existsSync(source)) throw new Error(`arkd sidecar was not built: ${source}`)

mkdirSync(dirname(destination), { recursive: true })
copyFileSync(source, destination)
