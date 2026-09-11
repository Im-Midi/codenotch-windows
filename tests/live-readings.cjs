const assert=require('node:assert/strict'),fs=require('node:fs');const {chromium}=require('playwright');
(async()=>{const b=await chromium.connectOverCDP('http://127.0.0.1:9227');try{
const p=b.contexts().flatMap(c=>c.pages()).find(p=>p.url().endsWith('/notch.html'));assert(p);
const errors=[];p.on('pageerror',e=>errors.push(e.message));
await p.locator('[data-p="codex"]').hover();
const result=await p.evaluate(()=>{const box=el=>{const r=el.getBoundingClientRect();return {x:r.x,y:r.y,width:r.width,height:r.height}};return {dpr:devicePixelRatio,pill:box(document.getElementById('pill')),card:box(document.getElementById('card')),cardShown:document.getElementById('card').classList.contains('show'),text:document.getElementById('card-content').innerText,viewport:{width:innerWidth,height:innerHeight},preview:!!document.documentElement.dataset.preview}});
assert(result.cardShown);assert(!result.preview);assert.match(result.text,/\d+% used/);assert(!/Demo data|Sample data/.test(result.text));
assert(result.card.x>=0&&result.card.y>=0&&result.card.x+result.card.width<=result.viewport.width+1&&result.card.y+result.card.height<=result.viewport.height+1);
await p.screenshot({path:'output/notch-live-expanded.png',omitBackground:true});assert.deepEqual(errors,[]);
delete result.text;fs.writeFileSync('output/live-layout-check.json',JSON.stringify(result,null,2));console.log('PASS: live Codex hover card, inward bounds, native bridge, no demo data or JS errors.');
}finally{await b.close();}})().catch(e=>{console.error(e);process.exitCode=1});
