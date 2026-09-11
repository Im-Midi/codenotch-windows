// Run against a separately launched --demo instance without --settings on 9228.
const assert = require('node:assert/strict');
const { chromium } = require('playwright');

(async () => {
  let browser;
  try {
    browser = await chromium.connectOverCDP(`http://127.0.0.1:${process.env.CDP_PORT || 9228}`);
    const context = browser.contexts()[0];
    const notch = context.pages().find(page => page.url().endsWith('/notch.html'));
    assert(notch, 'Launch Codenotch with --demo and remote debugging on 9228 first.');
    assert(!context.pages().some(page => page.url().endsWith('/settings.html')), 'Settings must be opened dynamically.');

    const settingsPage = context.waitForEvent('page');
    await notch.locator('#gear').click();
    const settings = await settingsPage;
    await settings.waitForLoadState('domcontentloaded');
    assert.match(await settings.locator('h1').innerText(), /Codenotch/);
    assert(await settings.locator('#close-settings').isVisible());
    assert(await settings.locator('#quit-app').isVisible());

    const closed = settings.waitForEvent('close');
    await settings.locator('#close-settings').click();
    await closed;
    assert(!context.pages().some(page => page.url().endsWith('/settings.html')), 'Close button did not close Settings.');
    const reopenedPage = context.waitForEvent('page');
    await notch.locator('#gear').click();
    const reopened = await reopenedPage;
    await reopened.waitForLoadState('domcontentloaded');
    const disconnected = new Promise(resolve => browser.once('disconnected', resolve));
    await reopened.locator('#quit-app').click();
    await Promise.race([disconnected, new Promise(resolve => setTimeout(resolve, 1500))]);
    assert.equal(browser.isConnected(), false, 'Quit button did not terminate the application.');
    console.log('PASS: dynamically opened Settings rendered, closed, reopened, and quit without freezing the app.');
  } finally {
    if (browser?.isConnected()) await browser.close().catch(() => {});
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
