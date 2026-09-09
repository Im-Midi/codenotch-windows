'use strict';
const {invoke}=window.__TAURI__.core, {listen}=window.__TAURI__.event;
const M=window.NotchModel, $=id=>document.getElementById(id);
const labels={codex:'Codex',claude:'Claude',cursor:'Cursor',gemini:'Antigravity'};
const edges=['right','left','top','bottom'];
let cfg, order=[], snaps={}, activity=[];
function message(text,error=false){$('message').textContent=text;$('message').classList.toggle('error',error);}
async function displays(){
  const value=$('monitor').value || cfg?.monitor_id || '';
  const list=await invoke('get_displays');
  $('monitor').replaceChildren(new Option('Primary display',''));
  for(const d of list)$('monitor').add(new Option(`${d.name} · ${d.bounds.width} × ${d.bounds.height}`,d.id));
  if(value&&!list.some(d=>d.id===value))$('monitor').add(new Option('Preferred display · disconnected',value));
  $('monitor').value=value;
}
function providers(){
  $('providers').innerHTML=order.map((id,i)=>`<div class="provider"><label class="check"><input type="checkbox" data-provider="${id}" ${cfg.providers.includes(id)?'checked':''}>${labels[id]}</label><button type="button" data-move="${id}" data-dir="-1" aria-label="Move ${labels[id]} up" ${i===0?'disabled':''}>↑</button><button type="button" data-move="${id}" data-dir="1" aria-label="Move ${labels[id]} down" ${i===order.length-1?'disabled':''}>↓</button></div>`).join('');
}
$('providers').addEventListener('change',e=>{const id=e.target.dataset.provider;if(!id)return;cfg.providers=e.target.checked?[...cfg.providers,id]:cfg.providers.filter(p=>p!==id);});
$('providers').addEventListener('click',e=>{const b=e.target.closest('[data-move]');if(!b)return;const i=order.indexOf(b.dataset.move),j=i+Number(b.dataset.dir);if(j<0||j>=order.length)return;[order[i],order[j]]=[order[j],order[i]];providers();$('providers').querySelector(`[data-move="${b.dataset.move}"][data-dir="${b.dataset.dir}"]`)?.focus();});
$('position').addEventListener('input',()=>{cfg.offsets[edges.indexOf($('edge').value)]=Number($('position').value)/100;$('position-value').textContent=$('position').value+'%';});
$('edge').addEventListener('change',()=>{$('position').value=Math.round(cfg.offsets[edges.indexOf($('edge').value)]*100);$('position-value').textContent=$('position').value+'%';});
$('settings').addEventListener('submit',async e=>{
  e.preventDefault();
  message('Saving settings...');
  const next={...cfg,monitor_id:$('monitor').value,edge:$('edge').value,scale:Number($('scale').value),providers:order.filter(id=>cfg.providers.includes(id)),codex_headline:$('headline').value,codex_home:$('profile').value.trim()};
  for(const key of ['avoid_taskbar','drag_enabled','always_on_top','auto_collapse','hide_fullscreen','reduced_motion'])next[key]=$(key).checked;
  try{await invoke('save_settings',{settings:next});cfg=next;message('Settings saved.');}catch(err){message(String(err),true);}
});
$('refresh').addEventListener('click',async()=>{try{await invoke('refresh_usage');message('Refresh requested. Provider cooldowns still apply.');}catch(e){message(String(e),true);}});
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&document.documentElement.dataset.preview!=='true')invoke('close_settings').catch(err=>message(String(err),true));});
function render(){
  if(!cfg)return;
  $('readings').innerHTML=cfg.providers.map(id=>{const s=snaps[id]||{windows:[]};return `<article class="reading"><h3>${labels[id]}</h3><div class="muted">${M.escape(s.note||'Waiting for first reading')} ${s.fetched_at?'· Updated '+new Date(s.fetched_at).toLocaleTimeString():''} ${M.stale(s)?'· Not current':''}</div>${s.status==='needsAuth'?'':(s.windows||[]).map(w=>`<div class="window">${M.escape(w.label)}: ${w.count!=null?'~'+w.count+' requests':M.percent(w.used)+'% used · '+(100-M.percent(w.used))+'% left'}${w.count==null?`<progress value="${M.percent(w.used)}" max="100" aria-label="${M.escape(w.label)} used"></progress>`:''}${w.resets_at?'<span class="muted">Resets '+M.escape(new Date(w.resets_at).toLocaleString())+'</span>':''}</div>`).join('')}${activity.filter(a=>a.provider===id).map(a=>`<div class="activity">${M.escape(a.name)} · ${M.escape(a.detail)}${a.inferred?' · inferred':''}</div>`).join('')}</article>`;}).join('')||'<p>No providers enabled.</p>';
}
(async()=>{
  cfg=await invoke('get_settings');order=[...cfg.providers,...Object.keys(labels).filter(id=>!cfg.providers.includes(id))];
  await displays();$('edge').value=cfg.edge;$('scale').value=cfg.scale;$('headline').value=cfg.codex_headline;$('profile').value=cfg.codex_home;
  for(const key of ['avoid_taskbar','drag_enabled','always_on_top','auto_collapse','hide_fullscreen','reduced_motion'])$(key).checked=cfg[key];
  $('position').value=Math.round(cfg.offsets[edges.indexOf(cfg.edge)]*100);$('position-value').textContent=$('position').value+'%';providers();
  for(const [id,event,cmd] of [['claude','usage','get_usage'],['codex','codex','get_codex'],['cursor','cursor','get_cursor'],['gemini','antigravity','get_antigravity']]){
    await listen(event,e=>{snaps[id]=e.payload;render();});snaps[id]=await invoke(cmd);
  }
  await listen('activity',e=>{activity=e.payload;render();});activity=await invoke('get_activity');
  await listen('displays_changed',()=>displays().catch(e=>message(String(e),true)));
  render();setInterval(render,30000);
})().catch(e=>message(String(e),true));
