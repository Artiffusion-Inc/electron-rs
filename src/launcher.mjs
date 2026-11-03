import { app } from 'electron'
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'

if (process.argv.slice(1).some(value => !value.startsWith('-psn_'))) {
  await import(pathToFileURL(join(process.resourcesPath, 'default_app.asar', 'main.js')).href)
} else {
  app.dock?.hide()
  let host = dirname(dirname(dirname(process.resourcesPath)))
  while (host !== dirname(host) && !(host.endsWith('.app') && existsSync(join(host, 'Contents', 'Info.plist')))) {
    host = dirname(host)
  }
  if (!host.endsWith('.app')) {
    console.error('ELECTRON_HOST_NOT_FOUND: launch the containing application')
    app.exit(1)
  } else {
    const result = spawnSync('/usr/bin/open', ['-a', host], { encoding: 'utf8', timeout: 10000 })
    if (result.error || result.status !== 0) {
      console.error('ELECTRON_HOST_LAUNCH_FAILED:', result.error?.message ?? result.stderr)
      app.exit(1)
    } else {
      app.exit(0)
    }
  }
}
