/**
 * Guard for scripts/change-impact.json (docs/requirements/ai-friendly-engineering-foundation.md).
 *
 * The impact matrix is only useful to a human or coding agent when it is
 * complete. This script makes "complete" executable: every workspace member in
 * the root Cargo.toml must be covered by at least one area path, every area
 * must name real design documents, and every area must declare a verification
 * command. Without it, a newly added crate can silently fall outside the map
 * and the next agent has no way to find its design or minimal test command.
 *
 * No third-party dependencies: run with `node scripts/check-change-impact.mjs`.
 */
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const failures = []

const cargoToml = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8')
const membersBlock = cargoToml.match(/members\s*=\s*\[([\s\S]*?)\]/)
if (!membersBlock) {
  console.error('[change-impact] cannot find workspace members in Cargo.toml')
  process.exit(1)
}
const members = [...membersBlock[1].matchAll(/"([^"]+)"/g)].map((m) => m[1])

const matrix = JSON.parse(readFileSync(join(ROOT, 'scripts/change-impact.json'), 'utf8'))
const areas = Array.isArray(matrix.areas) ? matrix.areas : []
for (const area of areas) {
  if (!area.id || !Array.isArray(area.paths) || !Array.isArray(area.design) || !Array.isArray(area.verify)) {
    failures.push(`area is missing id/paths/design/verify: ${JSON.stringify(area)}`)
    continue
  }
  if (area.verify.length === 0) failures.push(`[${area.id}] has no verification command`)
  for (const doc of area.design) {
    if (!existsSync(join(ROOT, doc))) failures.push(`[${area.id}] design document does not exist: ${doc}`)
  }
}

const covered = (member) =>
  areas.some((area) =>
    (area.paths || []).some((p) => member === p || member.startsWith(p) || p.startsWith(member))
  )
for (const member of members) {
  if (!covered(member)) failures.push(`workspace member has no change-impact area: ${member}`)
}

if (failures.length > 0) {
  console.error('[change-impact] failed:')
  for (const f of failures) console.error(`  - ${f}`)
  process.exit(1)
}
console.log(`[change-impact] OK: ${members.length} workspace members covered by ${areas.length} areas`)
