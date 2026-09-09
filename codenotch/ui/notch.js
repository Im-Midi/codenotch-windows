'use strict';
const {invoke}=window.__TAURI__.core,{listen}=window.__TAURI__.event,M=window.NotchModel;
const pill=document.getElementById('pill'),card=document.getElementById('card'),root=document.getElementById('root'),gear=document.getElementById('gear');
const labels={codex:'Codex',claude:'Claude',cursor:'Cursor',gemini:'Antigravity'},edges=['right','left','top','bottom'];
let cfg={edge:'right',scale:1,offsets:[.5,.5,.5,.5],providers:['codex','claude'],codex_headline:'weekly'},snaps={},glyphs={},activity=[],state={agg:'idle',sessions:[]};
let hoverId='codex',hideTimer,dragging=false,press=null,regionFrame,lastRegion='',ready=false;
const tone=f=>f>=.8?'#ff694f':f>=.5?'#f1dc62':'#76f49b';
const empty={status:'none',windows:[],note:'Waiting for first reading',fetched_at:0};
function providers(){return cfg.providers.filter(id=>snaps[id]?.status!=='absent');}
function icon(id){const g=glyphs[id];if(g?.kind==='svg'&&g.svg)return `<img class="provider-mark" alt="" src="data:image/svg+xml,${encodeURIComponent(g.svg).replace(/'/g,'%27')}">`;if(g?.url?.startsWith('data:image/'))return `<img alt="" src="${M.escape(g.url)}">`;return id==='codex'?'◎':id==='claude'?'✳':id==='cursor'?'C':'A';}
function acts(id){
  if(id==='claude'&&state.sessions?.length)return state.sessions.filter(s=>s.state!=='idle').map(s=>({name:s.title||'Claude',state:s.state==='running'?'busy':s.state==='attention'?'waiting':s.state,detail:s.state==='running'?'Working':s.state==='attention'?'Waiting for input':'Completed',since:s.updated_at||0,inferred:false}));
  return activity.filter(a=>a.provider===id);
}
function render(){
  const ids=providers(),key=ids.join(',')+Object.keys(glyphs).join(',');
  if(pill.dataset.cells!==key){
    pill.innerHTML=ids.map(id=>`<button class="cell" data-p="${id}" aria-label="${labels[id]} usage"><span class="ringwrap"><svg class="ring" viewBox="0 0 56 56" aria-hidden="true"></svg><span class="glyph">${icon(id)}</span></span><span class="pct">—</span></button>`).join('');pill.dataset.cells=key;
  }
  for(const id of ids){
    const s=snaps[id]||empty,h=M.headline(s,id==='codex'?cfg.codex_headline:'highest'),cell=pill.querySelector(`[data-p="${id}"]`),as=acts(id);
    const unavailable=s.status==='needsAuth';
    let svg='<circle cx="28" cy="28" r="25" fill="none" stroke="#292d29" stroke-width="5"/>';
    if(h&&h.count==null&&!unavailable)svg+=`<circle cx="28" cy="28" r="25" fill="none" stroke="${tone(h.used)}" stroke-width="5" stroke-dasharray="${Math.min(1,Math.max(0,h.used))*157.08} 157.08" stroke-linecap="round" transform="rotate(-90 28 28)"/>`;
    if(as.some(a=>a.state==='waiting'))svg+='<circle class="arc-pulse" cx="28" cy="28" r="19" fill="none" stroke="#ffbf00" stroke-width="2"/>';
    else if(as.some(a=>a.state==='busy'))svg+='<g class="arc-spin"><circle cx="28" cy="28" r="19" fill="none" stroke="#76f49b" stroke-width="2" stroke-dasharray="28 119"/></g>';
    // Keep running animations alive across quota refreshes.
    const ring=cell.querySelector('svg');if(ring.dataset.value!==svg){ring.innerHTML=svg;ring.dataset.value=svg;}
    const text=unavailable||!h?'—':h.count!=null?'~'+h.count:(h.derived?'~':'')+M.percent(h.used)+'%';
    cell.querySelector('.pct').textContent=text;cell.querySelector('.ringwrap').classList.toggle('dim',M.stale(s));
    cell.setAttribute('aria-label',`${labels[id]} ${h?.label||'usage'}: ${text}${M.stale(s)?', not current':''}`);
    cell.title=`${labels[id]} · ${h?.label||'usage unavailable'}`;
  }
  if(card.classList.contains('show'))renderCard();
  layout();
}
function age(ts){if(!ts)return 'Update time unknown';const seconds=Math.max(0,Math.floor((Date.now()-ts)/1000));return seconds<60?'just now':seconds<3600?Math.floor(seconds/60)+'m ago':Math.floor(seconds/3600)+'h ago';}
function reset(ts){if(!ts)return 'Reset unknown';return ts<=Date.now()?'Reset passed · awaiting refresh':'Resets '+new Date(ts).toLocaleString(undefined,{day:'numeric',month:'short',hour:'2-digit',minute:'2-digit'});}
function renderCard(){
  const id=providers().includes(hoverId)?hoverId:providers()[0];
  if(!id){card.classList.remove('show');return;}
  const s=snaps[id]||empty;let html=`<div class="c-head">${icon(id)}<span class="c-title">${labels[id]} Usage</span></div>`;
  if(s.status==='needsAuth'||!s.windows?.length)html+=`<div class="c-note">${M.escape(s.note||'Usage unavailable')}</div>`;
  else{
    for(const w of s.windows){const p=M.percent(w.used);html+=`<div class="win"><div class="w-row"><span class="w-label">${M.escape(w.label)}</span><span class="w-reset">${reset(w.resets_at)}</span></div>${w.count!=null?`<div class="w-used">~${w.count} requests</div>`:`<div class="w-track"><div class="w-fill" style="width:${p}%;background:${tone(w.used)}"></div></div><div class="w-used">${w.derived?'~':''}${p}% used · ${100-p}% left</div>`}</div>`;}
    if(s.note)html+=`<div class="c-note">${M.escape(s.note)}</div>`;
  }
  const active=acts(id);html+='<div class="c-sessions">';
  if(active.length)for(const a of active.slice(0,5))html+=`<div class="s-row"><span>${M.escape(a.name)}</span><span class="s-detail" style="color:${a.state==='waiting'?'#ffbf00':a.state==='busy'?'#76f49b':'#b1b7b2'}">${M.escape(a.detail)}${a.inferred?' · inferred':''}</span></div>`;
  else html+='<div class="s-row">Activity unknown / no active session detected</div>';
  html+=`<div class="s-time">${M.stale(s)?'Not current · ':''}Updated ${age(s.fetched_at)}</div></div>`;
  document.getElementById('card-content').innerHTML=html;
}
function layout(){
  const z=cfg.scale||1,W=innerWidth/z,H=innerHeight/z,edge=cfg.edge,vertical=edge==='left'||edge==='right';
  root.style.width=W+'px';root.style.height=H+'px';root.style.transform=`scale(${z})`;
  document.body.dataset.edge=edge;
  pill.style.width=vertical?'70px':Math.max(90,providers().length*56+Math.max(0,providers().length-1)*16+40)+'px';
  const pw=pill.offsetWidth,ph=pill.offsetHeight,ratio=cfg.offsets[edges.indexOf(edge)];
  // Leave room for the settings orb and the curved shoulders at both ends.
  const x=vertical?(edge==='right'?W-pw:0):M.clamp(W*ratio-pw/2,28,W-pw-66);
  const y=vertical?M.clamp(H*ratio-ph/2,28,H-ph-66):(edge==='bottom'?H-ph:0);
  Object.assign(pill.style,{left:x+'px',top:y+'px',right:'auto'});
  Object.assign(gear.style,{left:(vertical?x+(pw-34)/2:M.clamp(x+pw+12,0,W-34))+'px',top:(vertical?M.clamp(y+ph+12,0,H-34):y+(ph-34)/2)+'px'});
  if(card.classList.contains('show')){
    const cw=Math.max(80,Math.min(320,vertical?W-pw-24:W-16));card.style.width=cw+'px';card.style.maxHeight=Math.max(60,vertical?H-16:H-ph-24)+'px';
    const cell=pill.querySelector(`[data-p="${hoverId}"]`)||pill;
    const cr=cell.getBoundingClientRect(),cx=(cr.left+cr.width/2)/z,cy=(cr.top+cr.height/2)/z,ch=card.offsetHeight;
    const left=vertical?(edge==='right'?x-cw-16:x+pw+16):M.clamp(cx-cw/2,8,W-cw-8);
    const top=vertical?M.clamp(cy-ch/2,8,H-ch-8):(edge==='top'?y+ph+16:y-ch-16);
    Object.assign(card.style,{left:left+'px',top:top+'px',right:'auto'});
    card.style.setProperty('--tail',(vertical?M.clamp(cy-top,20,ch-20):M.clamp(cx-left,20,cw-20))+'px');
  }
  reportRegion();
}
function physical(el,radius){const r=el.getBoundingClientRect(),k=devicePixelRatio;return [r.x*k,r.y*k,r.width*k,r.height*k,radius*cfg.scale*k];}
function reportRegion(){
  cancelAnimationFrame(regionFrame);regionFrame=requestAnimationFrame(()=>{
    const edge=cfg.edge,k=devicePixelRatio*cfg.scale,r=physical(pill,56),rects=[r],collapsed=document.body.classList.contains('collapsed');
    if(!collapsed){
      // Square the screen-facing half of the rounded pill.
      if(edge==='right')rects.push([r[0]+r[2]/2,r[1],r[2]/2,r[3],0]);
      if(edge==='left')rects.push([r[0],r[1],r[2]/2,r[3],0]);
      if(edge==='top')rects.push([r[0],r[1],r[2],r[3]/2,0]);
      if(edge==='bottom')rects.push([r[0],r[1]+r[3]/2,r[2],r[3]/2,0]);
      const f=26*k;
      if(edge==='right')rects.push([r[0]+r[2]-f,r[1]-f,f,f,-5],[r[0]+r[2]-f,r[1]+r[3],f,f,-6]);
      if(edge==='left')rects.push([r[0],r[1]-f,f,f,-7],[r[0],r[1]+r[3],f,f,-8]);
      if(edge==='top')rects.push([r[0]-f,r[1],f,f,-6],[r[0]+r[2],r[1],f,f,-8]);
      if(edge==='bottom')rects.push([r[0]-f,r[1]+r[3]-f,f,f,-5],[r[0]+r[2],r[1]+r[3]-f,f,f,-7]);
      rects.push(physical(gear,34));
    }
    if(card.classList.contains('show')){
      const c=physical(card,44),t=parseFloat(card.style.getPropertyValue('--tail'))*k;rects.push(c);
      if(edge==='right')rects.push([c[0]+c[2],c[1]+t-8*k,10*k,16*k,-1]);
      if(edge==='left')rects.push([c[0]-10*k,c[1]+t-8*k,10*k,16*k,-2]);
      if(edge==='top')rects.push([c[0]+t-8*k,c[1]-10*k,16*k,10*k,-3]);
      if(edge==='bottom')rects.push([c[0]+t-8*k,c[1]+c[3],16*k,10*k,-4]);
    }
    const signature=JSON.stringify(rects);
    if(signature!==lastRegion){lastRegion=signature;invoke('set_expanded',{on:card.classList.contains('show'),rects}).catch(notice);}
    if(!ready){ready=true;invoke('ui_ready',{width:innerWidth,height:innerHeight,dpr:devicePixelRatio}).catch(()=>{});}
  });
}
function showCard(id){clearTimeout(hideTimer);document.body.classList.remove('collapsed');if(id)hoverId=id;card.classList.add('show');renderCard();layout();}
function hideCard(){card.classList.remove('show');document.body.classList.toggle('collapsed',cfg.auto_collapse&&!dragging);layout();}
function scheduleHide(){clearTimeout(hideTimer);hideTimer=setTimeout(hideCard,250);}
pill.addEventListener('pointerenter',()=>{document.body.classList.remove('collapsed');layout();});
pill.addEventListener('pointerover',e=>{const c=e.target.closest('[data-p]');if(c&&!dragging)showCard(c.dataset.p);});
pill.addEventListener('pointerleave',scheduleHide);card.addEventListener('pointerenter',()=>clearTimeout(hideTimer));card.addEventListener('pointerleave',scheduleHide);
gear.addEventListener('pointerenter',()=>clearTimeout(hideTimer));gear.addEventListener('pointerleave',scheduleHide);
gear.addEventListener('click',()=>invoke('open_settings').catch(notice));
pill.addEventListener('click',e=>{if(!dragging&&e.target.closest('[data-p]'))showCard(e.target.closest('[data-p]').dataset.p);});
pill.addEventListener('focusin',e=>{if(e.target.dataset.p)showCard(e.target.dataset.p);});
pill.addEventListener('pointerdown',e=>{if(e.button===0&&cfg.drag_enabled)press={x:e.clientX,y:e.clientY};});
document.addEventListener('pointermove',e=>{if(press&&!dragging&&Math.hypot(e.clientX-press.x,e.clientY-press.y)>4){dragging=true;hideCard();invoke('drag_begin').catch(e=>{dragging=false;notice(e);});}});
document.addEventListener('pointerup',()=>{press=null;});
document.addEventListener('keydown',e=>{if(e.key==='Escape')hideCard();});
function notice(text){const n=document.getElementById('notice');n.textContent=String(text);n.classList.add('show');setTimeout(()=>n.classList.remove('show'),6000);}
function apply(settings){cfg=settings;document.body.classList.toggle('reduced-motion',cfg.reduced_motion);if(!card.classList.contains('show'))document.body.classList.toggle('collapsed',cfg.auto_collapse&&!dragging);render();}
(async()=>{
  await listen('settings',e=>apply(e.payload));apply(await invoke('get_settings'));
  for(const [id,event,cmd] of [['claude','usage','get_usage'],['codex','codex','get_codex'],['cursor','cursor','get_cursor'],['gemini','antigravity','get_antigravity']]){
    await listen(event,e=>{snaps[id]=e.payload;render();});snaps[id]=await invoke(cmd);
  }
  await listen('glyphs',e=>{glyphs=e.payload;pill.dataset.cells='';render();});glyphs=await invoke('get_glyphs');
  await listen('activity',e=>{activity=e.payload;render();});activity=await invoke('get_activity');
  await listen('state',e=>{state=e.payload;render();});state=await invoke('get_state');
  await listen('drag_end',()=>{dragging=false;press=null;hideCard();});
  await listen('pointer_left',()=>{if(!dragging)hideCard();});
  await listen('notice',e=>notice(e.payload));
  render();
})().catch(notice);
window.addEventListener('resize',()=>{lastRegion='';layout();invoke('report_dpr',{dpr:devicePixelRatio,w:innerWidth,h:innerHeight}).catch(notice);});
setInterval(render,30000);
