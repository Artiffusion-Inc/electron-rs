import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import vm from 'node:vm'

const source = readFileSync(process.argv[2] ?? new URL('../../src/launcher.mjs', import.meta.url), 'utf8')
const root = mkdtempSync(path.join(tmpdir(), 'electron-launcher-'))
const host = path.join(root, 'DSH Tauri.app')
const resources = path.join(host, 'Contents/Resources/runtime/darwin-arm64/DSH Tauri.app/Contents/Resources')
mkdirSync(resources, { recursive: true })
writeFileSync(path.join(host, 'Contents/Info.plist'), '<plist/>')

async function launch(resourcesPath, argv = [], result = { status: 0 }, importGate = Promise.resolve()) {
  const events = []
  const context = vm.createContext({
    process: { resourcesPath, argv: ['Electron', ...argv] },
    console: { error: (...args) => events.push(['error', ...args.map(String)]) },
    resolveModule(name) {
      if (name === 'electron') return { app: { dock: { hide: () => events.push(['hide']) }, exit: code => events.push(['exit', code]) } }
      if (name === 'node:fs') return { existsSync: file => { try { readFileSync(file); return true } catch { return false } } }
      if (name === 'node:path') return path
      if (name === 'node:url') return { pathToFileURL }
      assert.equal(name, 'node:child_process')
      return { spawnSync: (file, args, options) => { events.push(['open', file, [...args], options.timeout]); return result } }
    },
  })
  const script = new vm.SourceTextModule(source, {
    context,
    importModuleDynamically: async specifier => {
      events.push(['import', specifier])
      await importGate
      return import('data:text/javascript,export default null')
    },
  })
  await script.link(specifier => {
    const value = context.resolveModule(specifier)
    return new vm.SyntheticModule(Object.keys(value), function () {
      for (const [key, item] of Object.entries(value)) this.setExport(key, item)
    }, { context })
  })
  await script.evaluate()
  return events
}

try {
  for (const argv of [[], ['-psn_0_12345']]) {
    assert.deepEqual(await launch(resources, argv), [
      ['hide'], ['open', '/usr/bin/open', ['-a', host], 10000], ['exit', 0],
    ])
  }
  for (const argv of [['/runtime/main.cjs', '--port', '1234'], ['--version'], ['-v']]) {
    assert.deepEqual(await launch(resources, argv), [
      ['import', pathToFileURL(path.join(resources, 'default_app.asar/main.js')).href],
    ], 'explicit scripts and diagnostics must keep the original Electron dispatcher')
  }
  let releaseImport, loaded = false
  const importGate = new Promise(resolve => { releaseImport = resolve })
  const pending = launch(resources, ['/runtime/main.cjs'], undefined, importGate).then(() => { loaded = true })
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(loaded, false, 'entry evaluation must wait for the original dispatcher before Electron becomes ready')
  releaseImport()
  await pending
  assert.equal(loaded, true)
  const failure = await launch(resources, [], { status: 1, stderr: 'open failed' })
  assert.deepEqual(failure.slice(-2), [['error', 'ELECTRON_HOST_LAUNCH_FAILED:', 'open failed'], ['exit', 1]])
  const detached = path.join(root, 'Detached.app/Contents/Resources')
  assert.deepEqual(await launch(detached), [
    ['hide'], ['error', 'ELECTRON_HOST_NOT_FOUND: launch the containing application'], ['exit', 1],
  ], 'a detached runtime must never relaunch itself')
} finally {
  rmSync(root, { recursive: true, force: true })
}
console.log('launcher: PASS (host handoff, original argument dispatch, launch failure, detached runtime)')
