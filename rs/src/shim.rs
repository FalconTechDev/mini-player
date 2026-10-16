//! Скрипты, внедряемые в окна.

/// Мост `pywebview.api.<метод>(...)` -> Promise: страницы остаются теми же, что в версии на Python.
pub const SHIM: &str = r#"(()=>{
 if(window!==window.top||window.__mpShim)return;window.__mpShim=1;
 const cb={};let n=0;
 window.__mpReply=(id,ok,v)=>{const c=cb[id];if(!c)return;delete cb[id];ok?c[0](v):c[1](new Error(String(v)))};
 const api=new Proxy({},{get:(t,name)=>{
  if(typeof name!=='string'||name==='then')return undefined;
  return (...args)=>new Promise((res,rej)=>{const id=++n;cb[id]=[res,rej];
   try{window.ipc.postMessage(JSON.stringify({id,name,args}))}catch(e){rej(e)}})}});
 window.pywebview={api};
 const fire=()=>window.dispatchEvent(new Event('pywebviewready'));
 if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',()=>setTimeout(fire,0));else setTimeout(fire,0);
})();"#;

/// Страницы считают, что окно всегда видно: видео не ставится на паузу при сворачивании.
pub const KEEPALIVE: &str = r#"(()=>{try{
 Object.defineProperty(Document.prototype,'hidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'visibilityState',{get:()=>'visible',configurable:true});
 Object.defineProperty(Document.prototype,'webkitHidden',{get:()=>false,configurable:true});
 Object.defineProperty(Document.prototype,'webkitVisibilityState',{get:()=>'visible',configurable:true});
 ['visibilitychange','webkitvisibilitychange'].forEach(t=>document.addEventListener(t,e=>e.stopImmediatePropagation(),true));
}catch(e){}})();"#;

/// Клик по видео на сайте YouTube/Twitch открывает его в плеере приложения.
pub fn intercept_js(kind: &str) -> String {
    format!(
        r#"(()=>{{const K='{kind}';if(window.__mpI)return;window.__mpI=1;
 const isVideo=h=>{{try{{const u=new URL(h,location.href);
  if(K==='yt')return /(^|\.)youtube\.com$/.test(u.hostname)&&(u.pathname==='/watch'&&u.searchParams.get('v')||/^\/(shorts|live)\/[\w-]{{11}}/.test(u.pathname));
  return /(^|\.)twitch\.tv$/.test(u.hostname)&&/^\/\w{{3,25}}\/?$/.test(u.pathname)&&!/^\/(directory|downloads|jobs|turbo|store|settings|subscriptions|inventory|wallet|drops|friends|messages|login|signup|search|p|videos|moderator|u)\/?$/i.test(u.pathname);}}catch(e){{return false}}}};
 document.addEventListener('click',e=>{{if(e.button!==0||e.ctrlKey||e.metaKey||e.shiftKey)return;
  const a=e.target.closest&&e.target.closest('a[href]');if(!a||!isVideo(a.href))return;
  e.preventDefault();e.stopPropagation();e.stopImmediatePropagation();
  if(window.pywebview&&pywebview.api)pywebview.api.open_video(a.href)}},true);
}})();"#
    )
}
