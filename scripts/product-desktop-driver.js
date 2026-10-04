// Injected only by the external acceptance runner, never compiled into Jev.
(async()=>{
 let step='desktop';
 try {
  const wait=async(fn)=>{const end=Date.now()+15000;while(Date.now()<end){const v=await fn();if(v)return v;await new Promise(r=>setTimeout(r,50));}throw new Error(`Timed out at ${step}`);};
  await wait(()=>document.querySelector('[data-nav-page="providers"]'));
  const invoke=window.__TAURI_INTERNALS__.invoke;
  const initial=await invoke('get_snapshot');
  let ordinary=false;try{await invoke('get_cpa_development_service');}catch(e){ordinary=String(e).includes('not found');}
  const test=window.__PRODUCT_TEST__;
  const set=async(element,value)=>{Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element),'value').set.call(element,value);element.dispatchEvent(new Event('input',{bubbles:true}));await new Promise(r=>setTimeout(r,80));};
  let service;
  if(test.cpa){
   document.querySelector('[data-nav-page="providers"]').click();
   if(!test.reopened){step='create';(await wait(()=>document.querySelector('.cpa-subscriptions form button'))).click();}
   const owner=await wait(()=>document.querySelector('.cpa-connection'));
   const start=()=>wait(()=>{const b=owner.querySelector('[data-testid^="cpa-owned-service-"] button');return !b.disabled&&b;});
   if(!test.reopened){
    step='wrong artifact';await set(owner.querySelector('[aria-label="专用 CPA 文件"]'),test.bad);(await start()).click();
    await wait(()=>document.body.innerText.includes('version/checksum mismatch'));
    step='occupied port';await set(owner.querySelector('[aria-label="专用 CPA 文件"]'),test.cpa);await set(owner.querySelector('[aria-label="专用 CPA 端口"]'),String(test.occupied));(await start()).click();
    await wait(()=>document.body.innerText.includes('port is already occupied'));
    await set(owner.querySelector('[aria-label="专用 CPA 端口"]'),'0');
   }
   step='start';await set(owner.querySelector('[aria-label="专用 CPA 文件"]'),test.cpa);(await start()).click();
   service=await wait(async()=>{const s=await invoke('get_snapshot');return s.cpa_subscriptions.find(c=>c.owned_service?.running);});
   if(service.stage!=='idle'||service.authorization_url||service.account||service.hand_run?.enabled)throw new Error('Starting a service must not authorize');
   step='stop';const stop=await wait(()=>{const b=owner.querySelectorAll('[data-testid^="cpa-owned-service-"] button')[1];return !b.disabled&&b;});stop.click();
   await wait(async()=>!(await invoke('get_snapshot')).cpa_subscriptions[0].owned_service.running);
   step='recover';(await start()).click();
   const recovered=await wait(async()=>{const c=(await invoke('get_snapshot')).cpa_subscriptions[0];return c.owned_service.running&&c;});
   step='owned exit';const killed=await fetch('/__kill_owned',{method:'POST',body:JSON.stringify({pid:recovered.owned_service.pid})});if(!killed.ok)throw new Error('Owner check failed');
   await wait(async()=>!(await invoke('get_snapshot')).cpa_subscriptions[0].owned_service.running);
   step='exit recovery';(await start()).click();
   const final=await wait(async()=>{const c=(await invoke('get_snapshot')).cpa_subscriptions[0];return c.owned_service.running&&c;});
   if(final.owned_service.port!==service.owned_service.port||final.connection_instance_id!==service.connection_instance_id||final.hand_run?.enabled)throw new Error('Recovery changed the fixed connection or enabled generation');
  }
  step='Coding Plan entry';
  if(!test.reopened){
   await invoke('save_provider',{provider:{id:'fictional-plan',name:'Fictional Coding Plan',kind:'openai_compatible',base_url:'http://127.0.0.1:1/coding/v4',api_type:'chat',enabled:true,has_api_key:false},apiKey:'fictional-plan-key',creating:true,source:{kind:'coding_plan',account_label:'fictional-account',plan_label:'fictional-plan'}});
   await invoke('save_model',{model:{...initial.models[0],id:'fictional-plan-model',model_id:'fictional-plan-model',name:'Fictional Plan Model',provider_id:'fictional-plan',api_type:'chat',enabled:true,selected:true}});
  }
  document.querySelector('[data-nav-page="providers"]').click();
  const pane=await wait(()=>document.querySelector('[data-coding-plan-id="fictional-plan"]'));
  if(!pane.innerText.includes('未审查')||!pane.innerText.includes('/coding/v4'))throw new Error('Coding Plan declarations became evidence');
  const enable=await wait(()=>{const b=pane.querySelector('button');return b&&!b.disabled&&b;});enable.click();
  await wait(()=>document.body.innerText.includes('Create the displayed owned evidence directory'));
  const plan=(await invoke('get_snapshot')).coding_hand_runs[0];if(plan.hand_run?.enabled)throw new Error('Missing evidence enabled plan');
  pane.scrollIntoView();const capture=await fetch('/__capture',{method:'POST',body:'{}'}).then(r=>r.json());
  await fetch('/__report',{method:'POST',body:JSON.stringify({ok:ordinary,initial,service,coding_plan:plan,capture,ordinary:true})});
  await invoke('quit_app');
 }catch(e){await fetch('/__report',{method:'POST',body:JSON.stringify({ok:false,error:String(e),step,body:document.body.innerText.slice(-3000)})});}
})();
