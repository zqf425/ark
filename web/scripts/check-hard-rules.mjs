/**
 * Local approximation of the CI gates in design §7.10.2.
 *
 * Runs without extra dependencies so it works before `npm install`. CI replaces
 * this with eslint + the generated-contract diff check, but the rules themselves
 * must be identical; this script is the executable statement of them.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative, sep } from 'node:path'

const ROOT = process.cwd()
const SRC = join(ROOT, 'src')
const MAX_LINES = 400

/** @type {{rule: string, file: string, detail: string}[]} */
const violations = []

function walk(dir) {
  /** @type {string[]} */
  const files = []
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry)
    const stat = statSync(full)
    if (stat.isDirectory()) files.push(...walk(full))
    else files.push(full)
  }
  return files
}

const files = walk(SRC)
const toPosix = (p) => relative(ROOT, p).split(sep).join('/')

for (const file of files) {
  const rel = toPosix(file)
  const source = readFileSync(file, 'utf8')
  const lines = source.split('\n')

  // Rule 1: single file <= 400 lines (eslint-max-lines equivalent).
  if (lines.length > MAX_LINES) {
    violations.push({ rule: 'max-lines', file: rel, detail: `${lines.length} 行 > ${MAX_LINES}` })
  }

  // Rule 2: no bare `fetch(` outside features/<x>/api and shared/api.
  const isApiLayer = /^src\/features\/[^/]+\/api\//.test(rel) || rel.startsWith('src/shared/api/')
  if (!isApiLayer && /\bfetch\s*\(/.test(source)) {
    violations.push({ rule: 'no-bare-fetch', file: rel, detail: '只允许 features/*/api 与 shared/api 调用 fetch' })
  }

  // Rule 3: only features/*/api may reference the raw runtime paths.
  if (!isApiLayer && /['"`]\/(v1|health)\//.test(source)) {
    violations.push({ rule: 'no-raw-runtime-path', file: rel, detail: '运行时路径只能出现在 api/ 层' })
  }

  // Rule 4: pages must not ship their own theme or toast implementation.
  const isSharedUi = rel.startsWith('src/shared/ui/')
  if (!isSharedUi && /new EventSource\(|createPortal\(/.test(source)) {
    violations.push({ rule: 'ui-primitives-only', file: rel, detail: '主题/弹窗/Toast 只能由 shared/ui 提供' })
  }
}

// Rule 5: contracts are generated artifacts (design §6.1 / §7.10.2).
const contractsDir = join(SRC, 'shared/contracts')
for (const file of walk(contractsDir)) {
  const source = readFileSync(file, 'utf8')
  if (file.endsWith('.ts') && !source.includes('待由 ark-contract 生成')) {
    violations.push({
      rule: 'generated-contract-header',
      file: toPosix(file),
      detail: '缺少“待由 ark-contract 生成”文件头'
    })
  }
}

if (violations.length > 0) {
  console.error('web 硬约束检查失败：')
  for (const v of violations) console.error(`  [${v.rule}] ${v.file} — ${v.detail}`)
  process.exit(1)
}

console.log(`web 硬约束检查通过（${files.length} 个文件）`)
