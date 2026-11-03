const { seeded, launch, homeIds } = require('./profile.cjs');
(async () => {
  const ctx = await launch(await seeded('main'), require('path').resolve('extension'));
  const page = ctx.pages()[0]; page.on('console', m => console.log('console', m.type(), m.text().slice(0, 300))); page.on('pageerror', e => console.log('pageerror', e.message));
  page.on('requestfinished', async r => { if (r.url().includes('/youtubei/v1/browse')) { const res = await r.response(); const b = (await res.body()).toString(); console.log('browse', res.status(), r.postData().slice(0, 900), '→', b.slice(0, 300)); } });
  await page.goto('https://www.youtube.com/', { waitUntil: 'commit' });
  await page.waitForTimeout(5000);
  console.log(await page.evaluate(() => [document.querySelectorAll('#ytc-list .c').length, document.querySelector('#ytc-main').textContent.slice(0, 200)]));
  await ctx.close();
})();
