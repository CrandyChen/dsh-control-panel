// Download helper: node scripts/download.mjs <url> <outPath>
// Uses Node's bundled OpenSSL (sandbox-friendly) instead of schannel.
import { writeFile, mkdir } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'

const [url, out] = process.argv.slice(2)
if (!url || !out) {
  console.error('usage: node scripts/download.mjs <url> <outPath>')
  process.exit(2)
}
const outPath = resolve(out)
await mkdir(dirname(outPath), { recursive: true })

const res = await fetch(url, {
  redirect: 'follow',
  signal: AbortSignal.timeout(30 * 60 * 1000),
})
if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText} for ${url}`)

const buf = Buffer.from(await res.arrayBuffer())
await writeFile(outPath, buf)
console.log(`saved ${buf.length} bytes -> ${outPath}`)
