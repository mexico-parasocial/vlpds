// Browser acceptance against the actual operator gateway, using synthetic users.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const require = createRequire(new URL('../../../WatZappa/packages/pds/package.json', import.meta.url))
const puppeteer = require('puppeteer')
const state = new URL('../.dev/chat-staging/', import.meta.url)
const browser = await puppeteer.launch({ headless: true })
try {
  const page = await browser.newPage()
  const failures = []
  page.on('pageerror', error => failures.push(error.message))
  await page.authenticate({ username: 'staging', password: (await readFile(new URL('secrets/console_staging_password', state), 'utf8')).trim() })
  await page.setViewport({ width: 1440, height: 1100 })
  await page.goto('http://localhost:38000/admin/chat', { waitUntil: 'domcontentloaded' })
  await page.waitForSelector('table[aria-label="Attested Matrix devices"]')
  const text = await page.$eval('body', node => node.innerText)
  assert(text.includes('Signed in as staging'))
  assert(text.includes('Durable relay checkpoint'))
  assert(/operator audit/i.test(text))
  const revoke = await page.$('table[aria-label="Attested Matrix devices"] button')
  if (revoke) {
    await revoke.click()
    await page.waitForSelector('input[minlength="5"]')
    assert(await page.$eval('input[minlength="5"]', node => node.required))
    const buttons = await page.$$('form button')
    await buttons[0].click() // Cancel the real device action.
  }
  await page.evaluate(() => document.querySelector('#cx-main')?.scrollTo(0, 0))
  await page.screenshot({ path: fileURLToPath(new URL('logs/chat-console.png', state)), fullPage: true })
  await page.setViewport({ width: 390, height: 844 })
  await page.screenshot({ path: fileURLToPath(new URL('logs/chat-console-mobile.png', state)), fullPage: true })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
  assert.deepEqual(failures, [])
  console.log('PASS rendered desktop/mobile console, live status, reason/cancel form and no browser exceptions')
  const limitedContext = await browser.createBrowserContext()
  const limited = await limitedContext.newPage()
  await limited.authenticate({ username: 'pds-only', password: (await readFile(new URL('secrets/console_pds-only_password', state), 'utf8')).trim() })
  await limited.goto('http://localhost:38000/admin/chat', { waitUntil: 'domcontentloaded' })
  await limited.waitForFunction(() => document.body.innerText.includes('Your operator sign-in does not permit'))
  assert.equal(await limited.$('table[aria-label="Attested Matrix devices"]'), null)
  console.log('PASS PDS-only operator sees a clear chat authorization error without device data')
} finally { await browser.close() }
