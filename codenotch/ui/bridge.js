// Only an explicitly requested browser preview can use sample data. Desktop pages require the native bridge.
if (!window.__TAURI__ && new URLSearchParams(location.search).get('preview') === '1' && !location.hostname.endsWith('tauri.localhost')) {
  const callbacks = {};
  const defaults = {edge:'right',monitor_id:'',offsets:[.5,.5,.5,.5],scale:1,avoid_taskbar:true,always_on_top:true,hide_fullscreen:true,auto_collapse:false,reduced_motion:false,drag_enabled:false,providers:['codex','claude'],codex_headline:'weekly',codex_home:'',port:48666,lang:'en'};
  const empty = {status:'absent',windows:[],fetched_at:0,note:''};
  const codex = {status:'ok',windows:[{id:'primary',label:'Weekly limit',used:.06,resets_at:Date.now()+6*86400000}],fetched_at:Date.now(),note:'Sample data'};
  window.__TAURI__ = {
    core:{invoke:async (cmd,args={}) => {
      switch (cmd) {
        case 'get_settings': return defaults;
        case 'get_displays': return [{id:'sample',name:'Sample display',bounds:{width:1920,height:1080},primary:true}];
        case 'save_settings': Object.assign(defaults,args.settings); (callbacks.settings||[]).forEach(fn=>fn({payload:defaults})); return;
        case 'get_codex': return codex;
        case 'get_usage': return {...empty,status:'none',note:'Sample data'};
        case 'get_cursor': case 'get_antigravity': return empty;
        case 'get_state': return {agg:'idle',sessions:[]};
        case 'get_activity': return [{provider:'codex',state:'busy',name:'Codex',detail:'Working',since:Date.now(),inferred:false}];
        case 'get_glyphs': return {};
        case 'open_settings': location.href='settings.html'; return;
      }
    }},
    event:{listen:async (name,fn)=>{(callbacks[name] ||= []).push(fn);return ()=>{};}}
  };
  document.documentElement.dataset.preview='true';
}
