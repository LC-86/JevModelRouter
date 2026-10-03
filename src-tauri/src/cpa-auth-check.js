// Native webview, public desktop actions and listening gateway; fictional CPA only.
(async()=>{
  if(window.__CPA_AUTH_RUNNING__)return;
  window.__CPA_AUTH_RUNNING__=true;
  const {base,run_id,reload}=window.__CPA_AUTH_CHECK__;
  const report={ok:false,run_id,layer:'native-desktop-fictional-cpa',checks:[]};
  const invoke=window.__TAURI_INTERNALS__.invoke;
  const check=(value,message)=>{if(!value)throw new Error(message);};
  const wait=async(fn,label)=>{const until=Date.now()+10000;while(Date.now()<until){const value=await fn();if(value)return value;await new Promise(r=>setTimeout(r,40));}throw new Error(`Timed out: ${label}`);};
  const row=()=>document.querySelector('[data-cpa-id]');
  const view=async()=> (await invoke('get_snapshot')).cpa_subscriptions[0];
  const post=async(path,body={})=> {const r=await fetch(base+path,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(body)});return r.json();};
  const button=async(label,scope=()=>document)=> {
    const b=await wait(()=>[...scope().querySelectorAll('button')].find(b=>b.textContent.trim()===label&&!b.disabled),label);
    b.click();
  };
  const stage=async(expected)=>wait(()=>row()?.querySelector(`[data-cpa-stage="${expected}"]`),expected);
  try {
    await wait(()=>document.querySelectorAll('.nav-item').length>1,'App ready');
    document.querySelectorAll('.nav-item')[1].click();
    await wait(()=>document.querySelector('[data-testid="cpa-subscriptions"]'),'CPA panel');
    if(reload) {
      const c=await view();check(c&&c.stage==='connected'&&c.account==='fictional-b','Saved account must survive restart without authorization');
      check(c.models.length===1&&c.models[0].selected&&c.models[0].bound,'Model identity/binding must persist');
      check(!c.service_available&&!c.authorization_url,'Restart never silently attaches a service or restarts login');
      report.saved={provider_id:c.provider_id,model_id:c.models[0].id,generation:c.generation};
      report.checks.push('isolated DB restart re-read; no automatic authorization');
    } else {
      await button('添加 CPA 连接');await stage('idle');
      await button('发起授权',row);await stage('waiting');
      await button('检查授权结果',row);await stage('waiting');
      await button('取消授权',row);await stage('cancelled');
      check(!(await view()).account,'Cancel clears identity');report.checks.push('begin/wait/cancel public desktop loop');
      await post('/__mode',{mode:'failed'});
      await button('发起授权',row);await stage('waiting');
      await button('检查授权结果',row);await stage('failed');
      await button('退出连接',row);await stage('disconnected');report.checks.push('failure/disconnect public desktop loop');
      await post('/__mode',{mode:'success',account:'fictional-a'});
      await button('发起授权',row);await stage('waiting');
      await button('检查授权结果',row);await stage('connected');
      await button('读取目录',row);await wait(()=>row().querySelector('[data-cpa-model]'),'directory');
      await button('重新绑定当前账号',row);
      let c=await wait(async()=>{const c=await view();return c.models[0]?.bound&&c;},'explicit model binding');
      const gateway=await invoke('start_proxy');
      await invoke('save_provider',{provider:{id:'same-name-paid',name:'Fictional paid API',kind:'openai_compatible',base_url:base,enabled:true,has_api_key:false,preset:'',api_type:'chat_completions',test_model:'same-model'},apiKey:'fictional-paid',creating:true,originalId:null,addTestModel:true});
      const savedModel=(await invoke('get_snapshot')).models.find(m=>m.id===c.models[0].id);
      for(const changedModel of [{...savedModel,provider_id:'same-name-paid',model_id:'other-model'}, {...savedModel,model_id:'other-model'}]) {
        let rejected=false;try {await invoke('save_model',{model:changedModel});}catch {rejected=true;}
        check(rejected,'A CPA fixed UUID must reject provider/upstream reassignment through public model editing');
      }
      await invoke('save_model',{model:{...savedModel,name:'用户显示名'}});
      check(!(await invoke('get_snapshot')).models.find(m=>m.id===savedModel.id).supports_tools,'Display edits must not invent CPA tools support');
      report.checks.push('public model edits retain the CPA fixed target and unknown capabilities');
      const reply=await post('/__gateway',{port:gateway.proxy.port,model:`autojev/model/${c.models[0].id}`,run_id});
      check(reply.status===429&&reply.body.error.code==='cpa_qualification_unknown','Login/directory must not authorize generation');
      report.checks.push('connection/directory/selection/listening gateway rejection; no same-name API fallback');
      await post('/__mode',{mode:'read-failed'});
      await button('读取目录',row);
      await wait(()=>row().textContent.includes('历史目录（陈旧）'),'stale history');
      check((await view()).models[0].id===c.models[0].id,'Failed directory read preserves stable ID');
      await post('/__mode',{mode:'success',account:'fictional-b'});
      await button('更换账号',row);await stage('waiting');
      await button('检查授权结果',row);await stage('connected');
      const changed=await view();check(changed.account==='fictional-b'&&!changed.models[0].bound,'New account never inherits old fixed target');
      const old=await post('/__gateway',{port:gateway.proxy.port,model:`autojev/model/${c.models[0].id}`,run_id});
      check(old.status===403&&old.body.error.code==='cpa_target_identity_changed','Old fixed target must fail after switch');
      await button('读取目录',row);
      await button('重新绑定当前账号',row);
      c=await wait(async()=>{const c=await view();return c.models[0]?.bound&&c;},'rebind account B');
      report.saved={provider_id:c.provider_id,model_id:c.models[0].id,generation:c.generation};
      report.checks.push('stale directory retained; switch disables old target; explicit rebind');
    }
    const metadata=(await invoke('get_snapshot')).models.find(m=>m.id===report.saved.model_id);
    check(!metadata.supports_tools&&!metadata.supports_vision&&!metadata.supports_reasoning&&metadata.context_window===0,'Discovery/edit/reload must keep unverified capabilities unknown');
    const receipts=await post('/__receipts');
    check(receipts.models===0,'No model request may reach CPA or same-name API');
    check(receipts.downloads===0&&receipts.accountQueries===0,'No auth export/account/quota queries');
    report.receipts=receipts;report.ok=true;
    await post('/__capture',{run_id});
  } catch(error){report.error=String(error);}
  await invoke('isolation_check_report',{report});
})();
