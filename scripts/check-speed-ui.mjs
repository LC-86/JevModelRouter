import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
const browser = await chromium.launch({headless:true});
try {
  const page = await browser.newPage({viewport:{width:1400,height:1000},locale:'zh-CN'});
  const errors=[]; page.on('pageerror',e=>errors.push(e.message));
  // Exercise UI state transitions with a fake measurement service; never call paid providers.
  await page.route('**/src/lib/bridge.ts*', async route => {
    const response=await route.fetch();
    let body=await response.text();
    const start=body.indexOf('export async function getModelPerformance(');
    assert.ok(start>0);
    const tail=body.indexOf('export async function executeDebugCurl',start);
    assert.ok(tail>start,'Bridge exports after the speed-test functions must remain available to the app.');
    body=body.slice(0,start)+`
      const testView={settings:{enabled:true,interval_minutes:30},models:{
        'qwen-fast':{first_content_ms:230,tokens_per_second:74.2,success_rate:1,samples:5,last_test_at:Date.now(),stale:false},
        'claude-sonnet-4':{first_content_ms:null,tokens_per_second:null,success_rate:0,samples:0,last_test_at:Date.now()-3600000,stale:true}
      },job:{running:false,cancelled:false,completed:0,total:0,completed_models:0,total_models:0,current_models:[],error:null}};
      export async function getModelPerformance(){return structuredClone(testView);}
      export async function startModelSpeedTests(ids){testView.job={running:true,cancelled:false,completed:0,total:ids.length*3,completed_models:0,total_models:ids.length,current_models:ids,error:null};}
      export async function cancelModelSpeedTests(){testView.job.running=false;testView.job.cancelled=true;testView.job.current_models=[];}
      export async function savePerformanceSettings(settings){testView.settings=settings;}
    `+body.slice(tail);
    await route.fulfill({response,body});
  });
  await page.goto(process.env.AUTOJEV_PREVIEW_URL || 'http://127.0.0.1:1433');
  await page.getByRole('button',{name:'模型',exact:true}).click();
  await page.getByText(/230 ms/).waitFor();
  await page.getByText('测速数据已过期',{exact:true}).waitFor();
  assert.equal(await page.getByRole('columnheader',{name:'连接状态',exact:true}).count(),0);
  assert.equal(await page.getByText('补测间隔',{exact:true}).count(),0);
  assert.ok(await page.getByRole('button',{name:'测速',exact:true}).isEnabled());
  await page.getByRole('button',{name:'测速',exact:true}).click();
  await page.getByText(/API 请求最多要求输出 24 个词元，订阅响应限制为 64 KiB/).waitFor();
  const selectionError = page.locator('.toast.toast-error').filter({hasText:'请先勾选要测速的模型。'});
  await selectionError.waitFor();
  assert.equal(await page.getByRole('dialog').count(),0);
  await selectionError.waitFor({state:'hidden',timeout:5000});
  await page.getByRole('button',{name:'测速',exact:true}).click();
  await selectionError.waitFor();
  assert.equal(await page.getByRole('button',{name:'停止测速',exact:true}).count(),0);
  await page.getByRole('checkbox',{name:'选择全部已启用模型',exact:true}).check();
  await page.getByRole('button',{name:/测选中模型/}).click();
  await page.getByRole('button',{name:'停止测速',exact:true}).waitFor();
  assert.equal(await page.locator('.models-table .provider-connection.testing').count(),2);
  assert.ok((await page.locator('.model-speed-count').innerText()).includes('已完成请求 0/6'));
  assert.equal(await page.getByRole('button',{name:/测选中模型/}).count(),0);
  await page.getByRole('button',{name:'停止测速',exact:true}).click();
  await page.getByRole('button',{name:/测选中模型/}).waitFor();
  await page.getByText('已停止：完成 0/6 次请求。',{exact:true}).waitFor();
  await mkdir('/tmp/autojev-speed-ui',{recursive:true});
  await page.screenshot({path:'/tmp/autojev-speed-ui/models.png',fullPage:true});
  await page.locator('.settings-trigger').click();
  await page.getByRole('button',{name:'通用',exact:true}).click();
  assert.equal(await page.getByRole('switch',{name:'自动测速',exact:true}).count(),0);
  await page.getByRole('button',{name:'网关与路由',exact:true}).click();
  await page.getByRole('tab',{name:'模型测速',exact:true}).click();
  assert.equal(await page.getByRole('switch',{name:'自动测速',exact:true}).getAttribute('aria-checked'),'true');
  await page.getByLabel('补测间隔').click();
  await page.getByRole('option',{name:'15 分钟',exact:true}).click();
  await page.getByRole('switch',{name:'自动测速',exact:true}).click();
  await page.keyboard.press('Escape');
  await page.locator('.settings-trigger').click();
  await page.getByRole('tab',{name:'模型测速',exact:true}).click();
  await page.getByRole('switch',{name:'自动测速',exact:true}).waitFor();
  assert.equal(await page.getByRole('switch',{name:'自动测速',exact:true}).getAttribute('aria-checked'),'false');
  assert.ok((await page.getByLabel('补测间隔').innerText()).includes('15'));
  await page.screenshot({path:'/tmp/autojev-speed-ui/settings.png',fullPage:true});
  await page.keyboard.press('Escape');
  await page.getByRole('button',{name:'路由',exact:true}).click();
  await page.getByRole('button',{name:'添加路由',exact:true}).click();
  await page.getByLabel('调度方式').click();
  await page.getByRole('option',{name:'智能选择',exact:true}).click();
  await page.getByLabel('决策偏好').click();
  await page.getByRole('option',{name:'速度优先',exact:true}).click();
  await page.getByText(/根据近期实测速度选择模型/).waitFor();
  assert.equal(await page.getByLabel('优先使用本地模型').count(),0);
  await page.screenshot({path:'/tmp/autojev-speed-ui/route.png',fullPage:true});
  await page.getByRole('button',{name:'取消',exact:true}).click();
  await page.getByRole('button',{name:'调试台',exact:true}).click();
  await page.getByText(/Debug 请求与普通请求共用生成及订阅准入/).waitFor();
  assert.deepEqual(errors,[]);
  console.log('Speed UI passed: measurements, stale data, selection, running/cancellation, schedule settings, local speed route.');
} finally {await browser.close();}
