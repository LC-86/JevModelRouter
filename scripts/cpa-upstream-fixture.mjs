// Both acceptance layers use this fake upstream; CPA itself is always the pinned executable.
export function respondFromFictionalAccount(res, account, stream, mode = 'ok') {
  if (mode === 'drop') { res.destroy(); return; }
  if (Number.isInteger(mode)) {
    res.writeHead(mode, {'content-type':'application/json', 'retry-after':'60'});
    res.end(JSON.stringify({error:{message:`fixture ${mode}`,type:'fixture_error'}})); return;
  }
  if (stream) {
    res.writeHead(200, {'content-type':'text/event-stream'});
    res.end('data: '+JSON.stringify({id:'fixture',object:'chat.completion.chunk',model:'same-model',choices:[{index:0,delta:{role:'assistant',content:account},finish_reason:null}]})+'\n\ndata: '+JSON.stringify({id:'fixture',choices:[{index:0,delta:{},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
  } else {
    res.writeHead(200, {'content-type':'application/json'});
    res.end(JSON.stringify({id:'fixture',object:'chat.completion',model:'same-model',choices:[{index:0,message:{role:'assistant',content:account},finish_reason:'stop'}],usage:{prompt_tokens:1,completion_tokens:1,total_tokens:2}}));
  }
}
