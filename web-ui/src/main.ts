import './style.css'

const $ = (s: string) => document.querySelector(s) as HTMLElement
// 默认空走 vite proxy /api→8132 避 CORS，直接填 8132 则直连（需后端 CORS）
const api = () => {
  const v = ($('#apiBase') as HTMLInputElement).value.trim().replace(/\/$/, '')
  if (!v || v === 'http://127.0.0.1:8132' || v === 'http://localhost:8132') return ''
  return v
}

function toast(msg: string, err=false) {
  const t = $('#toast')!
  t.textContent = msg
  t.style.background = err ? '#dc2626' : '#16a34a'
  t.style.opacity = '1'
  setTimeout(()=> t.style.opacity='0', 2500)
}
async function jget(p:string){ const r=await fetch(api()+p); if(!r.ok) throw new Error(await r.text()); return r.json()}
async function jpost(p:string,b?:any){ const r=await fetch(api()+p,{method:'POST',headers:{'Content-Type':'application/json'},body: b?JSON.stringify(b):'{}'}); const t=await r.text(); let d:any; try{d=JSON.parse(t)}catch{d={raw:t}}; if(!r.ok) throw new Error(t); return d}
async function jput(p:string,b:any){ const r=await fetch(api()+p,{method:'PUT',headers:{'Content-Type':'application/json'},body: JSON.stringify(b)}); const t=await r.text(); let d:any; try{d=JSON.parse(t)}catch{d={raw:t}}; if(!r.ok) throw new Error(t); return d}
async function jdel(p:string){ const r=await fetch(api()+p,{method:'DELETE'}); const t=await r.text(); if(!r.ok) throw new Error(t); return t}

function renderTabs(){
  const tabs = ['drivers','devices','endpoints','points','diagnostics','certs']
  const nav = $('#tabs')!
  nav.innerHTML = tabs.map(t=>`<button data-tab="${t}" class="tab">${t}</button>`).join('')
  nav.querySelectorAll('.tab').forEach(b=>{
    b.addEventListener('click',()=>{
      nav.querySelectorAll('.tab').forEach(x=>x.classList.remove('active'))
      b.classList.add('active')
      document.querySelectorAll('.pane').forEach(p=>p.classList.remove('show'))
      $('#pane-'+(b as HTMLElement).dataset.tab!)?.classList.add('show')
    })
  })
  ;(nav.querySelector('.tab') as HTMLElement)?.click()
}

async function loadDrivers(){
  try{
    const d=await jget('/api/v1/drivers')
    $('#drivers').innerHTML = `<pre>${JSON.stringify(d,null,2)}</pre><button id="rescan">rescan</button>`
    $('#rescan')?.addEventListener('click', async()=>{ await jpost('/api/v1/drivers/rescan'); toast('rescan ok'); loadDrivers() })
  }catch(e:any){ $('#drivers').innerHTML=`<span class="err">${e.message}</span>`}
}
async function loadDevices(){
  try{
    const d=await jget('/api/v1/devices')
    void (d.devices||d||[])
    $('#devices').innerHTML = `<div class="row"><input id="devId" placeholder="id dev-001"><input id="devName" placeholder="name"><button id="addDev">+ device</button></div><pre>${JSON.stringify(d,null,2)}</pre>`
    $('#addDev')?.addEventListener('click', async()=>{
      const id=( $('#devId') as HTMLInputElement).value.trim(); const name=( $('#devName') as HTMLInputElement).value.trim()||id
      if(!id) return toast('id required',true)
      await jpost('/api/v1/devices',{id,name}); toast('device created'); loadDevices()
    })
  }catch(e:any){ $('#devices').innerHTML=`<span class="err">${e.message}</span>`}
}
async function loadEndpoints(){
  try{
    const d=await jget('/api/v1/endpoints')
    const eps = d.endpoints||[]
    let html = `<div class="card"><h3>创建 Endpoint</h3>
      <input id="epId" placeholder="ep-001"> <input id="epDev" placeholder="device_id dev-001"> 
      <select id="epDrv"><option value="simulator">simulator</option><option value="s7">s7</option><option value="focas2">focas2</option><option value="opcua">opcua</option></select><br>
      <textarea id="epConn" rows="3" placeholder='connection JSON e.g. {"host":"192.168.15.165","port":8193,"timeout_ms":3000,"use_native":true} or {"endpoint_url":"opc.tcp://127.0.0.1:4840"}'></textarea><br>
      <button id="addEp">+ endpoint</button></div>`
    html += eps.map((e:any)=>`<div class="card"><b>${e.id}</b> ${e.driver_id} ${e.device_id} <span class="badge ${e.runtime?.state}">${e.runtime?.state||e.state||''}</span> epoch:${e.runtime?.epoch||''} pts:${e.runtime?.points||0}<br><small>${JSON.stringify(e.connection)}</small><br>
      <button data-start="${e.id}">start</button> <button data-stop="${e.id}">stop</button> <button data-del="${e.id}">delete</button> <button data-tasks="${e.id}">tasks</button></div>`).join('')
    $('#endpoints').innerHTML = html + `<pre>${JSON.stringify(d,null,2)}</pre>`
    $('#addEp')?.addEventListener('click', async()=>{
      const id=( $('#epId') as HTMLInputElement).value.trim(), device_id=( $('#epDev') as HTMLInputElement).value.trim(), driver_id=( $('#epDrv') as HTMLInputElement).value
      let connection:any={}; try{ connection=JSON.parse(( $('#epConn') as HTMLTextAreaElement).value||'{}')}catch{return toast('connection JSON invalid',true)}
      if(!id||!device_id) return toast('id/device_id required',true)
      await jpost('/api/v1/endpoints',{id,device_id,driver_id,connection}); toast('endpoint created'); loadEndpoints()
    })
    $('#endpoints').querySelectorAll('[data-start]').forEach(b=> b.addEventListener('click', async()=>{ const id=(b as HTMLElement).dataset.start!; await jpost(`/api/v1/endpoints/${id}/start`); toast('started'); loadEndpoints()}))
    $('#endpoints').querySelectorAll('[data-stop]').forEach(b=> b.addEventListener('click', async()=>{ const id=(b as HTMLElement).dataset.stop!; await jpost(`/api/v1/endpoints/${id}/stop`); toast('stopped'); loadEndpoints()}))
    $('#endpoints').querySelectorAll('[data-del]').forEach(b=> b.addEventListener('click', async()=>{ const id=(b as HTMLElement).dataset.del!; await jdel(`/api/v1/endpoints/${id}`); toast('deleted'); loadEndpoints()}))
    $('#endpoints').querySelectorAll('[data-tasks]').forEach(b=> b.addEventListener('click', ()=> openTasks((b as HTMLElement).dataset.tasks!)))
  }catch(e:any){ $('#endpoints').innerHTML=`<span class="err">${e.message}</span>`}
}
function openTasks(epId:string){
  const taskJson = prompt(`Tasks JSON for ${epId}\n示例 focas 35点 / s7 DB10.DBD0 / opcua ns=2;i=1`, `{"tasks":[{"id":"t","mode":"poll","interval_ms":500,"binding":{"kind":"focas.data-block","config":{"items":[{"key":"status","address":"status","data_type":"U32"},{"key":"axis1","address":"axis.abs.1","data_type":"I32"}]}}}]} `)
  if(!taskJson) return
  let body:any; try{ body=JSON.parse(taskJson)}catch{ return toast('JSON invalid',true)}
  // Stop → PUT → Start 流程 §6.2
  jpost(`/api/v1/endpoints/${epId}/stop`).catch(()=>{}).finally(async()=>{
    try{ await jput(`/api/v1/tasks/${epId}`, body); toast('tasks put ok, starting...'); await jpost(`/api/v1/endpoints/${epId}/start`); toast('started with new tasks'); loadEndpoints()
    }catch(e:any){ toast(e.message,true)}
  })
}
let pointsTimer:any=null
async function loadPoints(){
  try{
    const d=await jget('/api/v1/points/latest')
    const pts=d.points||[]
    $('#points').innerHTML = `<div class="row"><label><input type="checkbox" id="autoPts" checked> auto 1s</label> <span>${pts.length} points</span></div><table><tr><th>endpoint</th><th>key</th><th>point_id</th><th>quality</th><th>type</th><th>value</th><th>time</th></tr>${pts.map((p:any)=>`<tr><td>${p.endpoint_id}</td><td>${p.key}</td><td>${p.point_id}</td><td class="${p.quality}">${p.quality}</td><td>${p.type}</td><td class="val">${String(p.value).slice(0,120)}</td><td>${new Date(p.timestamp_ns/1e6).toLocaleTimeString()}</td></tr>`).join('')}</table><pre>${JSON.stringify(d,null,2).slice(0,3000)}</pre>`
    const auto = $('#autoPts') as HTMLInputElement
    if(auto?.checked){
      if(pointsTimer) clearTimeout(pointsTimer)
      pointsTimer = setTimeout(loadPoints, 1000)
    }
  }catch(e:any){ $('#points').innerHTML=`<span class="err">${e.message}</span>`}
}
async function loadDiagnostics(){
  try{
    const d=await jget('/api/v1/diagnostics')
    const c=await jget('/api/v1/certificates/opcua/diagnostics').catch(()=>({}))
    $('#diagnostics').innerHTML = `<h3>diagnostics</h3><pre>${JSON.stringify(d,null,2)}</pre><h3>cert diagnostics</h3><pre>${JSON.stringify(c,null,2)}</pre>`
  }catch(e:any){ $('#diagnostics').innerHTML=`<span class="err">${e.message}</span>`}
}
async function loadCerts(){
  const stores=['own','trusted','issuers','rejected'] as const
  let html='<div class="row">'
  for(const s of stores){ html+=`<button data-store="${s}">${s}</button>`}
  html+=`</div><div id="certList"></div><div class="card"><h3>导入 trusted</h3><textarea id="pemIn" rows="4" placeholder="-----BEGIN CERTIFICATE-----"></textarea><br><button id="addPem">import trusted</button></div>`
  $('#certs').innerHTML = html
  for(const b of $('#certs').querySelectorAll('[data-store]')) b.addEventListener('click', async()=>{
    const s=(b as HTMLElement).dataset.store!
    try{
      const d=await jget(`/api/v1/certificates/opcua/${s}`)
      let listHtml = `<h3>${s}</h3><pre>${JSON.stringify(d,null,2).slice(0,4000)}</pre>`
      if(s==='trusted' || s==='rejected'){
        const certs = d.certs||d.certificates||d||[]
        const arr = Array.isArray(certs)?certs: (certs.certs||[])
        if(Array.isArray(arr)) listHtml += arr.map((c:any)=>`<div class="card"><small>${c.thumbprint||c.thumb||''} ${c.subject||''}</small><br><button data-del="${c.thumbprint||''}">delete</button> ${s==='rejected'?`<button data-trust="${c.thumbprint||''}">trust</button>`:''}</div>`).join('')
      }
      $('#certList').innerHTML = listHtml
      $('#certList').querySelectorAll('[data-del]').forEach(x=> x.addEventListener('click', async()=>{ const t=(x as HTMLElement).dataset.del!; await jdel(`/api/v1/certificates/opcua/trusted/${t}`); toast('deleted'); (b as HTMLElement).click()}))
      $('#certList').querySelectorAll('[data-trust]').forEach(x=> x.addEventListener('click', async()=>{ const t=(x as HTMLElement).dataset.trust!; await jpost(`/api/v1/certificates/opcua/rejected/${t}/trust`); toast('trusted'); (b as HTMLElement).click()}))
    }catch(e:any){ $('#certList').innerHTML=`<span class="err">${e.message}</span>`}
  })
  $('#addPem')?.addEventListener('click', async()=>{
    const pem=( $('#pemIn') as HTMLTextAreaElement).value.trim(); if(!pem) return toast('pem required',true)
    await jpost('/api/v1/certificates/opcua/trusted',{pem}); toast('imported')
  })
}

document.querySelector<HTMLDivElement>('#app')!.innerHTML = `
<header><h1>Mesa</h1><small>V1 MVP 8132 loopback · 空走 proxy 避 CORS</small>
  <div class="row"><input id="apiBase" value="" placeholder="留空走 proxy /api→8132 或填 http://127.0.0.1:8132" style="width:320px"> <button id="btnReload">↻ Reload</button> <span id="toast" style="margin-left:12px;opacity:0;transition:.3s;padding:4px 8px;border-radius:4px;color:#fff"></span></div>
  <nav id="tabs" class="tabs"></nav>
</header>
<main>
  <section id="pane-drivers" class="pane"><h2>Drivers</h2><div id="drivers"></div></section>
  <section id="pane-devices" class="pane"><h2>Devices</h2><div id="devices"></div></section>
  <section id="pane-endpoints" class="pane"><h2>Endpoints</h2><div id="endpoints"></div></section>
  <section id="pane-points" class="pane"><h2>Points Latest <small>stream_epoch seq quality Bad隔离</small></h2><div id="points"></div></section>
  <section id="pane-diagnostics" class="pane"><h2>Diagnostics</h2><div id="diagnostics"></div></section>
  <section id="pane-certs" class="pane"><h2>OPC UA Certificates <small>§8 pki_dir own/trusted 0o600</small></h2><div id="certs"></div></section>
</main>
<footer><small>Mesa Driver MVP · Rust + Tokio + Protobuf IPC + SQLite · V1 只读 · 8132 loopback</small></footer>
`
renderTabs()
$('#btnReload')?.addEventListener('click', ()=>{ loadDrivers(); loadDevices(); loadEndpoints(); loadPoints(); loadDiagnostics(); })
loadDrivers(); loadDevices(); loadEndpoints(); loadPoints(); loadDiagnostics(); loadCerts()

// 初始加载后每 5s 刷新 endpoints 状态
setInterval(()=>{ if($('#pane-endpoints')?.classList.contains('show')) loadEndpoints() }, 5000)
