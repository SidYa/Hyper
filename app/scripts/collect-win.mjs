// Copies the NSIS installer of `pnpm app:build` to target/win/, next to
// target/linux/ (build-linux.sh), so every release file sits in one place.
// Older installers there are removed first. Does nothing off Windows: the
// Linux builds run `app:build` too and collect their own files.
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

if (process.platform !== 'win32') process.exit(0)

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), '..', '..')
const nsis = join(repoRoot, 'target', 'release', 'bundle', 'nsis')
const out = join(repoRoot, 'target', 'win')

const setups = existsSync(nsis) ? readdirSync(nsis).filter((f) => f.endsWith('-setup.exe')) : []
if (!setups.length) {
  console.error(`no NSIS installer in ${nsis}`)
  process.exit(1)
}
mkdirSync(out, { recursive: true })
for (const f of readdirSync(out).filter((f) => f.endsWith('-setup.exe'))) rmSync(join(out, f))
for (const f of setups) {
  copyFileSync(join(nsis, f), join(out, f))
  console.log(`-> ${join(out, f)}`)
}
