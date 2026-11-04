import {test} from 'node:test';import assert from 'node:assert/strict';import {spawn} from 'node:child_process';import {createInterface} from 'node:readline';import path from 'node:path';
import {moduleUrl,drain} from './module-loader.mjs';
const id='1'.repeat(32),bytes=Array.from({length:9017},(_,i)=>(i%100-50)&255);
function engine(){
 const child=spawn('target/debug/examples/protocol_bridge',{stdio:['pipe','pipe','inherit']}),lines=createInterface({input:child.stdout}),queue=[],waiters=[];
 lines.on('line',line=>{const value=JSON.parse(line);if(waiters.length)waiters.shift()(value);else queue.push(value)});
 return {async request(value){child.stdin.write(JSON.stringify(value)+'\n');return queue.length?queue.shift():new Promise(resolve=>waiters.push(resolve))},close(){child.stdin.end()}};
}
function storage(){const mem={};return {mem,file:{
 readText:o=>o.uri in mem?o.success({text:mem[o.uri]}):o.fail('missing',301),writeText:o=>{mem[o.uri]=o.text;o.success()},get:o=>o.uri in mem?o.success({length:mem[o.uri].length}):o.fail('missing',301),delete:o=>{delete mem[o.uri];o.success()},move:o=>{mem[o.dstUri]=mem[o.srcUri];delete mem[o.srcUri];o.success()},readArrayBuffer:o=>o.success({buffer:mem[o.uri].slice(o.position,o.position+o.length)}),writeArrayBuffer:o=>{mem[o.uri]=Uint8Array.from([...(o.append?mem[o.uri]:[]),...o.buffer]);o.success()}}}}
const start={command:'start',id,title:'原生 V3 测试',bytes};
test('10p: enhanced PCM is saved exactly with peak 127 in the verified receipt',async()=>{
 const {createMusicReceiver}=await import(moduleUrl(path.resolve('tests/fixtures/receivers/10p/music.js'))),{mem,file}=storage(),rx=createMusicReceiver(file,{}, {setTimeout:()=>0,clearTimeout:()=>{}}),e=engine();
 const enhanced=Array.from({length:9017},(_,i)=>(i%255-127)&255);
 try{let s=await e.request({...start,bytes:enhanced,peak:127}),loops=0;
  while(s.phase!=='saved'&&loops++<100){s=s.pending?await e.request({command:'ack',ack:await rx.handle(s.pending)}):await e.request({command:'timer'})}
  assert.equal(s.phase,'saved',s.status);assert.equal(s.receipt.peak,127);assert.equal(s.receipt.id,undefined);
  assert.deepEqual(mem['internal://files/music_'+id+'.bin'],Uint8Array.from(enhanced));
 }finally{e.close();rx.close()}
});
for(const model of ['10','10p','9p'])test(model+': Rust sender saves exact PCM and queries lost final receipt',async()=>{
 const {createMusicReceiver}=await import(moduleUrl(path.resolve('tests/fixtures/receivers/'+model+'/music.js'))),{mem,file}=storage(),rx=createMusicReceiver(file,{}, {setTimeout:()=>0,clearTimeout:()=>{}}),e=engine();
 try{let s=await e.request(start),loops=0;while(!['saved','paused','unconfirmed'].includes(s.phase)&&loops++<100){
  if(s.pending){const a=await rx.handle(s.pending);s=await e.request(s.pending.op==='commit'?{command:'timer'}:{command:'ack',ack:a})}else s=await e.request({command:'timer'});
 }assert.equal(s.phase,'saved',s.status);assert.equal(s.receipt.verified,true);assert.equal(s.receipt.id,undefined);assert.deepEqual(mem['internal://files/music_'+id+'.bin'],Uint8Array.from(bytes));
 }finally{e.close();rx.close()}
});

for(const model of ['10','10p','9p'])test(model+': new transfers do not leave persistent checkpoints or reuse old ids',async()=>{
 const {createMusicReceiver}=await import(moduleUrl(path.resolve('tests/fixtures/receivers/'+model+'/music.js')));
 const {mem,file}=storage(),timers={setTimeout:()=>0,clearTimeout:()=>{}},old=engine(),fresh=engine();
 let rx=createMusicReceiver(file,{},timers);
 try{
  let s=await old.request(start);
  assert.equal(s.pending.resumeRevision,undefined,'plugin must not request durable resume');
  s=await old.request({command:'ack',ack:await rx.handle(s.pending)});
  await rx.handle(s.pending);
  assert.equal(mem['internal://files/music_pending.json'],undefined);
  rx.close();await drain();
  assert.equal(mem['internal://files/music_'+id+'.bin.tmp'],undefined,'closed partial task is temporary');
  rx=createMusicReceiver(file,{},timers);
  s=await fresh.request({...start,id:'2'.repeat(32)});
  const a=await rx.handle(s.pending);assert.equal(a.ok,true,a.error);assert.equal(a.next,0);
  assert.equal(s.pending.id,'2'.repeat(32));
 }finally{old.close();fresh.close();rx.close()}
});

test('10p: an existing legacy checkpoint is reported without adopting or deleting its id',async()=>{
 const {createMusicReceiver}=await import(moduleUrl(path.resolve('tests/fixtures/receivers/10p/music.js')));
 const {mem,file}=storage(),timers={setTimeout:()=>0,clearTimeout:()=>{}},e=engine();let rx=createMusicReceiver(file,{},timers);
 try{
  let s=await e.request(start),legacy={...s.pending,id:'3'.repeat(32),resumeRevision:1};
  assert.equal((await rx.handle(legacy)).ok,true);rx.close();await drain();
  const checkpoint=mem['internal://files/music_pending.json'];
  rx=createMusicReceiver(file,{},timers);
  s=await e.request({command:'ack',ack:await rx.handle(s.pending)});
  assert.equal(s.phase,'blocked');assert.equal(s.pending,null);
  assert.equal(mem['internal://files/music_pending.json'],checkpoint);
  assert.ok(s.status.includes('旧续传记录'));
 }finally{e.close();rx.close()}
});

const pendingUri='internal://files/music_pending.json';
async function legacyReceiver(){
 const {createMusicReceiver}=await import(moduleUrl(path.resolve('tests/fixtures/receivers/10p/music.js')));
 const {mem,file}=storage(),timers={setTimeout:()=>0,clearTimeout:()=>{}},e=engine();
 const s=await e.request(start);e.close();
 const legacy={...s.pending,id:'3'.repeat(32),resumeRevision:1};
 let rx=createMusicReceiver(file,{},timers);assert.equal((await rx.handle(legacy)).ok,true);
 rx.close();await drain();
 return {mem,file,timers,legacy,createMusicReceiver};
}
test('10p: local cleanup discovers an old checkpoint without a supplied id and preserves saved songs',async()=>{
 const {mem,file,timers,legacy,createMusicReceiver}=await legacyReceiver();
 const song={...legacy,id:'4'.repeat(32)};
 mem['internal://files/music_library.json']=JSON.stringify({version:2,items:[song]});
 const savedUri='internal://files/music_'+song.id+'.bin',saved=Uint8Array.from(bytes);mem[savedUri]=saved;
 const rx=createMusicReceiver(file,{},timers);
 try{
  const result=await rx.cancelPendingTransfer();assert.equal(result.cancelled,true);assert.equal(result.id,undefined);
  assert.equal(mem[pendingUri],undefined);assert.equal(mem['internal://files/music_'+legacy.id+'.bin.tmp'],undefined);
  assert.deepEqual(mem[savedUri],saved);assert.equal(JSON.parse(mem['internal://files/music_library.json']).items.length,1);
  const e=engine();try{const s=await e.request(start);assert.equal((await rx.handle(s.pending)).ok,true)}finally{e.close()}
 }finally{rx.close()}
});
test('10p: local cleanup is serialized against new transfers and repeated clicks',async()=>{
 const {mem,file,timers,legacy,createMusicReceiver}=await legacyReceiver();
 const original=file.readText;let held;
 file.readText=o=>o.uri===pendingUri&&!held?(held=o):original(o);
 const rx=createMusicReceiver(file,{},timers);
 try{
  const first=rx.cancelPendingTransfer(),second=rx.cancelPendingTransfer();await drain();
  assert.equal(rx.busy,true);assert.ok(held);
  assert.equal(await rx.handle({...legacy,id:'5'.repeat(32)}),null);
  held.success({text:mem[pendingUri]});
  assert.equal((await first).cancelled,true);assert.equal((await second).cancelled,true);assert.equal(rx.busy,false);
  assert.equal((await rx.cancelPendingTransfer()).empty,true);
 }finally{rx.close()}
});
test('10p: local cleanup keeps published output even when a stale checkpoint remains',async()=>{
 const {mem,file,timers,legacy,createMusicReceiver}=await legacyReceiver();
 mem['internal://files/music_library.json']=JSON.stringify({version:2,items:[legacy]});
 const savedUri='internal://files/music_'+legacy.id+'.bin';mem[savedUri]=Uint8Array.from(bytes);
 const rx=createMusicReceiver(file,{},timers);
 try{assert.equal((await rx.cancelPendingTransfer()).saved,true);assert.deepEqual(mem[savedUri],Uint8Array.from(bytes));assert.equal(mem[pendingUri],undefined)}finally{rx.close()}
});
test('10p: failed or corrupt local checkpoint reads preserve all files',async()=>{
 const {mem,file,timers,legacy,createMusicReceiver}=await legacyReceiver();mem[pendingUri]='invalid';
 const before={...mem},rx=createMusicReceiver(file,{},timers);
 try{assert.equal((await rx.cancelPendingTransfer()).ok,false);assert.deepEqual(mem,before)}finally{rx.close()}
});
