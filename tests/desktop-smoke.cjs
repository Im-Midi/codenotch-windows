// Run against a separately launched --demo instance with remote debugging on 9227.
// Uses Playwright's library, not a test framework. Never launch with real credentials for this check.
const assert=require('node:assert/strict');
const fs=require('node:fs');
const {chromium}=require('playwright');
(async()=>{
  const browser=await chromium.connectOverCDP('http://127.0.0.1:9227');
  const pages=browser.contexts().flatMap(c=>c.pages());
  const settings=pages.find(p=>p.url().endsWith('/settings.html'));
  const notch=pages.find(p=>p.url().endsWith('/notch.html'));
  assert(settings&&notch,'Launch Codenotch with --demo --settings first.');
  const errors=[];for(const p of pages){p.on('pageerror',e=>errors.push(e.message));p.on('console',m=>{if(m.type()==='error')errors.push(m.text());});}
  assert.match(await settings.locator('#readings').innerText(),/Demo data/,'Refusing to change settings outside demo mode.');
  const original=await settings.evaluate(()=>window.__TAURI__.core.invoke('get_settings'));
  const displays=await settings.locator('#monitor option').evaluateAll(opts=>opts.map(o=>({id:o.value,label:o.textContent})).filter(o=>o.id));
  const results=[];
  async function save(){
    await settings.getByRole('button',{name:'Save settings',exact:true}).click();
    // Wait for the native command and its rendered result, not a fixed timeout.
    await settings.waitForFunction(()=>document.getElementById('message').textContent==='Settings saved.');
    assert.equal(await settings.locator('#message').evaluate(el=>el.classList.contains('error')),false);
  }
  try{
    for(const name of ['Codex','Claude'])await settings.getByRole('checkbox',{name,exact:true}).check();
    for(const name of ['Cursor','Antigravity'])await settings.getByRole('checkbox',{name,exact:true}).uncheck();
    await settings.getByRole('checkbox',{name:'Reduce motion',exact:true}).check();
    await settings.getByRole('checkbox',{name:'Collapse to a small tab when not in use',exact:true}).uncheck();
    for(const display of displays){
      await settings.getByLabel('Display',{exact:true}).selectOption(display.id);
      for(const edge of ['right','left','top','bottom']){
        await settings.getByLabel('Edge',{exact:true}).selectOption(edge);
        for(const scale of ['0.75','1','1.25','1.5']){
          await settings.getByLabel('Size',{exact:true}).selectOption(scale);await save();
          await notch.waitForFunction(({edge,scale})=>document.body.dataset.edge===edge&&document.getElementById('root').style.transform===`scale(${scale})`,{edge,scale});
          const rect=await notch.locator('#pill').boundingBox();const viewport=await notch.evaluate(()=>({w:innerWidth,h:innerHeight}));
          assert(rect&&rect.x>=-1&&rect.y>=-1&&rect.x+rect.width<=viewport.w+1&&rect.y+rect.height<=viewport.h+1,JSON.stringify({edge,scale,rect,viewport}));
          assert.equal(await notch.locator('.cell').count(),2);
          results.push({monitor:display.label,edge,scale,pass:true});
        }
      }
    }
    fs.writeFileSync('output/desktop-matrix.json',JSON.stringify({matrix:results,passed:true},null,2));
    // Position endpoints, order and collapse behavior use the same saved UI controls.
    await settings.getByLabel('Display',{exact:true}).selectOption('');
    await settings.getByLabel('Edge',{exact:true}).selectOption('right');
    await settings.getByLabel('Size',{exact:true}).selectOption('1');
    for(const key of ['Home','End']){
      await settings.getByRole('slider').press(key);await save();
      await notch.getByRole('button',{name:/Codex Weekly/}).hover();
      const box=await notch.locator('#card').boundingBox(),v=await notch.evaluate(()=>({w:innerWidth,h:innerHeight}));
      assert(box,'Hover card is visible');assert(box.x>=0&&box.y>=0&&box.x+box.width<=v.w+1&&box.y+box.height<=v.h+1);
    }
    await settings.getByRole('slider').press('Home');for(let i=0;i<5;i++)await settings.getByRole('slider').press('PageUp');
    await settings.getByRole('button',{name:'Move Codex down',exact:true}).click();await save();
    await notch.waitForFunction(()=>document.querySelector('.cell')?.dataset.p==='claude');
    await settings.getByRole('button',{name:'Move Codex up',exact:true}).click();await save();
    await settings.getByRole('checkbox',{name:'Collapse to a small tab when not in use',exact:true}).check();await save();
    await notch.mouse.move(0,0);await notch.waitForFunction(()=>document.body.classList.contains('collapsed'));
    await settings.getByRole('checkbox',{name:'Collapse to a small tab when not in use',exact:true}).uncheck();await save();
    await notch.getByRole('button',{name:/Codex Weekly/}).hover();
    await notch.screenshot({path:'output/notch-native.png',omitBackground:true});
    await settings.screenshot({path:'output/settings-native.png'});
    assert.deepEqual(errors,[],'WebView errors');
    fs.writeFileSync('output/desktop-smoke.json',JSON.stringify({matrix:results,checks:['position endpoints','provider order','collapse','CSP and JS errors'],passed:true},null,2));
    console.log(`PASS: ${results.length} native WebView display/edge/size combinations, provider order, collapse, and position endpoints.`);
  }finally{
    await settings.evaluate(cfg=>window.__TAURI__.core.invoke('save_settings',{settings:cfg}),original);
    await browser.close();
  }
})().catch(e=>{console.error(e);process.exitCode=1;});
