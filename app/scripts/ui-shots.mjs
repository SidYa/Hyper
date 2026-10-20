// Screenshots / scripted checks of the UI preview (preview.html, mocked
// backend) in a headless Edge or Chromium, over the DevTools protocol.
// Needs the Vite dev server: `pnpm -C app dev` (http://localhost:1420).
//
//   node app/scripts/ui-shots.mjs <plan.json> <out-dir> [query] [WxH]
//   node app/scripts/ui-shots.mjs plan.json shots "lang=uk&stores=epic" 1280x800
//
// plan.json is a list of steps, run in order:
//   {"key": "ArrowDown", "wait": 400}   press + release (Arrow*, Enter, Escape, PageUp/Down)
//   {"down": "Escape"} / {"up": "Escape"} hold and release a key
//   {"eval": "js expression", "print": "label"}  evaluate (awaited), print the result
//   {"shot": "name"}                    save <out-dir>/name.png
//   {"wait": 1000}
// Page errors and console warnings are printed at the end. BROWSER overrides
// the browser binary; PREVIEW_URL the page (default the dev server).
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const [planFile, out, query = '', size = '1280x800'] = process.argv.slice(2)
if (!planFile || !out) {
  console.error('usage: node ui-shots.mjs <plan.json> <out-dir> [query] [WxH]')
  process.exit(2)
}
const [width, height] = size.split('x').map(Number)
const plan = JSON.parse(fs.readFileSync(planFile, 'utf8'))
const url = `${process.env.PREVIEW_URL ?? 'http://localhost:1420/preview.html'}${query ? `?${query}` : ''}`
fs.mkdirSync(out, { recursive: true })

const candidates = [
  process.env.BROWSER,
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  '/usr/bin/microsoft-edge',
  '/usr/bin/chromium',
  '/usr/bin/google-chrome'
].filter(Boolean)
const browser = candidates.find((p) => fs.existsSync(p))
if (!browser) {
  console.error('no Edge / Chromium found; set BROWSER')
  process.exit(1)
}

const port = 9300 + Math.floor(Math.random() * 600)
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'hyper-shots-'))
const proc = spawn(browser, [`--headless=new`, `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, `--window-size=${width},${height}`, '--autoplay-policy=no-user-gesture-required', 'about:blank'], { stdio: 'ignore' })

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
let targets
for (let i = 0; i < 50 && !targets; i++) {
  await sleep(200)
  targets = await fetch(`http://127.0.0.1:${port}/json`).then((r) => r.json(), () => undefined)
}
const page = targets?.find((t) => t.type === 'page')
if (!page) {
  proc.kill()
  console.error('browser did not start')
  process.exit(1)
}

const ws = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((r) => ws.addEventListener('open', r))
let id = 0
const pending = new Map()
const logs = []
ws.addEventListener('message', (m) => {
  const msg = JSON.parse(m.data)
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg)
    pending.delete(msg.id)
  }
  if (msg.method === 'Runtime.exceptionThrown') logs.push('exception ' + (msg.params.exceptionDetails.exception?.description ?? msg.params.exceptionDetails.text))
  if (msg.method === 'Runtime.consoleAPICalled' && ['error', 'warning'].includes(msg.params.type)) {
    logs.push(msg.params.type + ' ' + msg.params.args.map((a) => a.value ?? a.description).join(' '))
  }
})
const send = (method, params = {}) =>
  new Promise((r) => {
    const i = ++id
    pending.set(i, r)
    ws.send(JSON.stringify({ id: i, method, params }))
  })

const KEY_CODES = { ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, Enter: 13, Escape: 27, PageUp: 33, PageDown: 34, ' ': 32 }
const key = (type, k) => send('Input.dispatchKeyEvent', { type, key: k, code: k, windowsVirtualKeyCode: KEY_CODES[k] })

try {
  await send('Runtime.enable')
  await send('Page.enable')
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false })
  await send('Page.navigate', { url })
  await sleep(2500)
  for (const step of plan) {
    if (step.down || step.up) {
      await key(step.down ? 'keyDown' : 'keyUp', step.down ?? step.up)
      await sleep(step.wait ?? 100)
    } else if (step.key) {
      await key('keyDown', step.key)
      await key('keyUp', step.key)
      await sleep(step.wait ?? 400)
    } else if (step.eval) {
      const r = await send('Runtime.evaluate', { expression: step.eval, awaitPromise: true, returnByValue: true })
      if (step.print) console.log(step.print, JSON.stringify(r.result?.result?.value ?? r.result?.exceptionDetails?.exception?.description))
      await sleep(step.wait ?? 400)
    } else if (step.shot) {
      const r = await send('Page.captureScreenshot', { format: 'png' })
      fs.writeFileSync(path.join(out, `${step.shot}.png`), Buffer.from(r.result.data, 'base64'))
      console.log('shot', path.join(out, `${step.shot}.png`))
    } else if (step.wait) await sleep(step.wait)
  }
  if (logs.length) console.log(logs.slice(0, 30).join('\n'))
} finally {
  ws.close()
  // Browser.close ends every browser process; kill() alone stops only the
  // parent on Windows and leaves its helpers running (and the profile locked)
  const exited = new Promise((r) => proc.once('exit', r))
  try {
    const { webSocketDebuggerUrl } = await fetch(`http://127.0.0.1:${port}/json/version`).then((r) => r.json())
    const bws = new WebSocket(webSocketDebuggerUrl)
    await new Promise((r, e) => {
      bws.addEventListener('open', r)
      bws.addEventListener('error', e)
    })
    bws.send(JSON.stringify({ id: 1, method: 'Browser.close' }))
  } catch {
    proc.kill()
  }
  await Promise.race([exited, sleep(5000)])
  await sleep(500)
  try {
    fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 300 })
  } catch {
    console.warn(`could not remove the temporary profile ${profile}`)
  }
}
