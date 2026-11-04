import { parseLyrics } from './lyrics.js'
// HMP1: one acknowledged, bounded block at a time. No native playback here.
export function createMusicReceiver(file, hooks={}, timers={}) {
  const base='internal://files/', tag='haptic.music.v1'
  let catalogSnapshot=null
  const later=timers.setTimeout || setTimeout, cancel=timers.clearTimeout || clearTimeout
  let closed=false, working=false, operation=null, current=null, saved=[], last=null, idle=null, restoring=null, generation=0
  let libraryWriteUnconfirmed=null,builtinRemoved=false
  let cancelledId=null, pendingId=null, pendingCancelled=false
  let loaded=false, garbage=[], done=Promise.resolve(), finishWork=null, cancelling=null
  const table=[]
  for(let i=0;i<256;i++){let c=i;for(let j=0;j<8;j++)c=(c>>>1)^((c&1)?0xedb88320:0);table[i]=c>>>0}
  const update=(c,b)=>{for(let i=0;i<b.length;i++)c=(c>>>8)^table[(c^b[i])&255];return c>>>0}
  const valid=m=>m&&/^[a-f0-9]{32}$/.test(m.id)&&Number.isInteger(m.bytes)&&m.bytes>0&&m.bytes<=3600000&&
    Number.isInteger(m.crc32)&&m.crc32>=0&&m.crc32<=0xffffffff&&m.rate===12000&&m.encoding==='s8'&&Number.isInteger(m.peak)&&m.peak>=1&&m.peak<=127&&
    typeof m.title==='string'&&m.title.length>0&&m.title.length<=64&&!/[\x00-\x1f]/.test(m.title)
  const status=s=>{if(!closed&&hooks.status)hooks.status(s)}
  const call=(method,args)=>new Promise((resolve,reject)=>{
    let done=false,settle
    const pending=new Promise(resolve=>{settle=resolve})
    const finish=(e,r)=>{if(done)return;done=true;cancel(t);e?reject(e):resolve(r)}
    const t=later(()=>{const e=new Error(method+' 回调超时，结果未确认');e.unconfirmed=true;e.pending=pending;finish(e)},15000)
    try{file[method](Object.assign({},args,{success:r=>{settle();finish(null,r)},fail:(e,c)=>{const error=new Error(method+' '+c+' '+String(e));error.code=c;settle();finish(error)}}))}catch(e){settle();finish(e)}
  })
  const check=()=>{if(closed)throw new Error('接收页已关闭');if(current&&current.cancelled&&!current.publishing)throw new Error('传输已取消')}
  const remove=uri=>call('delete',{uri}).catch(()=>{})
  function arm(){if(idle!==null)cancel(idle);idle=later(()=>{if(working){arm();return}const c=current;current=null;last=null;if(c&&!c.resumable)remove(c.temp);status(c&&c.resumable?'传输已暂停，可在手机继续':'传输中断，请在手机重新发送')},60000)}
  function disarm(){if(idle!==null)cancel(idle);idle=null}
  function track(m){return {id:m.id,uri:base+'music_'+m.id+'.bin',bytes:m.bytes,crc32:m.crc32,title:m.title,
    lyrics:typeof m.lyrics==='string'?m.lyrics:'',coverUri:Number.isInteger(m.coverSlot)?coverUri(m.id,m.coverSlot):'',
    rate:12000,encoding:'s8',peak:m.peak,group:'phone',tone:'手机音乐 · '+(m.bytes/12000).toFixed(1)+'秒'}}
  const pendingUri=base+'music_pending.json'
  const sameContent=(a,b)=>['id','title','bytes','crc32','rate','encoding','peak'].every(k=>a[k]===b[k])&&(a.lyrics||'')===(b.lyrics||'')
  async function checkpoint(c,pending){
    if(!c.resumable)return
    const record={revision:1,meta:c.meta,transport:c.transport,offset:c.offset,seq:c.seq,pending:pending||null}
    await call('writeText',{uri:pendingUri+'.tmp',text:JSON.stringify(record),append:false})
    await call('move',{srcUri:pendingUri+'.tmp',dstUri:pendingUri})
  }
  async function readCheckpoint(){
    let r
    try{r=JSON.parse((await call('readText',{uri:pendingUri})).text)}catch(e){if(e.code===301)return null;throw e}
    if(!r||r.revision!==1||!valid(r.meta)||!r.transport||!Number.isInteger(r.transport.chunkBytes)||r.transport.chunkBytes<256||r.transport.chunkBytes>16384||
      !Number.isInteger(r.offset)||r.offset<0||r.offset>r.meta.bytes||!Number.isInteger(r.seq)||r.seq<1||
      (r.offset!==r.meta.bytes&&r.offset%r.transport.chunkBytes)||r.seq!==Math.ceil(r.offset/r.transport.chunkBytes)+1)throw new Error('续传记录无效，已保留暂存文件')
    return r
  }
  async function clearCheckpoint(){for(const uri of [pendingUri,pendingUri+'.tmp']){try{await call('delete',{uri})}catch(e){holdUnconfirmed(e);if(e.code!==301)throw e}}}
  async function recover(m,transport){
    const r=await readCheckpoint();if(!r)return null
    if(!sameContent(r.meta,m)||JSON.stringify(r.transport)!==JSON.stringify(transport))throw new Error('存在另一首未完成传输，请先继续或中断原任务')
    const c={meta:r.meta,transport:r.transport,offset:r.offset,seq:r.seq,temp:base+'music_'+m.id+'.bin.tmp',resumable:true}
    let info
    try{info=await call('get',{uri:c.temp})}catch(e){
      if(e.code!==301)throw e
      // The audio rename may have completed before catalog publication. Restore its staging name.
      info=await call('get',{uri:track(c.meta).uri})
      if(info.length!==c.meta.bytes||c.offset!==c.meta.bytes)throw new Error('续传音频长度无效')
      await call('move',{srcUri:track(c.meta).uri,dstUri:c.temp})
    }
    if(info.length!==c.offset){
      const x=r.pending
      if(!x||!Number.isInteger(x.bytes)||x.bytes<1||x.bytes>transport.chunkBytes||info.length<c.offset||info.length>c.offset+x.bytes||info.length>c.meta.bytes)throw new Error('未完成分块长度异常，请中断后重新发送')
      const b=new Uint8Array((await call('readArrayBuffer',{uri:c.temp,position:c.offset,length:info.length-c.offset})).buffer)
      if(b.length===x.bytes&&((update(0xffffffff,b)^0xffffffff)>>>0)===x.crc32){c.offset+=x.bytes;c.seq++}
      else{
        // Only a crash in a native write needs this bounded prefix repair; normal reconnects
        // read metadata and at most the pending block, never the entire audio.
        const repair=c.temp+'.repair';await remove(repair)
        await call('writeArrayBuffer',{uri:repair,buffer:new Uint8Array(0),append:false,position:0})
        for(let p=0;p<c.offset;p+=16384){
          const n=Math.min(16384,c.offset-p),part=new Uint8Array((await call('readArrayBuffer',{uri:c.temp,position:p,length:n})).buffer)
          if(part.length!==n)throw new Error('续传暂存文件已损坏')
          await call('writeArrayBuffer',{uri:repair,buffer:part,append:p>0,position:0})
        }
        await call('move',{srcUri:repair,dstUri:c.temp})
      }
    }
    await checkpoint(c);return c
  }
  const catalogRows=()=>saved.map(x=>({id:x.id,title:x.title,bytes:x.bytes,crc32:x.crc32,hasLyrics:!!x.lyrics,lyricsVersion:Number.isInteger(x.lyricsVersion)?x.lyricsVersion:0,
    hasCover:Number.isInteger(x.coverSlot),coverVersion:Number.isInteger(x.coverVersion)?x.coverVersion:0}))
  const orderCrc=ids=>(update(0xffffffff,ids.join('').split('').map(c=>c.charCodeAt(0)))^0xffffffff)>>>0
  function catalogPage(m,reply){
    if(m.catalogPageRevision!==1){reply.songs=catalogRows();return}
    const offset=m.catalogOffset===undefined?0:m.catalogOffset
    if(!Number.isInteger(offset)||offset<0)throw new Error('清单分页位置无效')
    if(offset===0)catalogSnapshot={id:m.id,generation,rows:catalogRows()}
    const snapshot=catalogSnapshot
    if(!snapshot||snapshot.id!==m.id||snapshot.generation!==generation||offset>snapshot.rows.length||
      (offset>0&&m.catalogToken!==String(snapshot.generation)))throw new Error('歌曲清单已改变，请重新读取')
    const end=Math.min(offset+20,snapshot.rows.length)
    reply.catalogPageRevision=1;reply.catalogOffset=offset;reply.catalogTotal=snapshot.rows.length
    reply.catalogToken=String(snapshot.generation);reply.catalogNext=end<snapshot.rows.length?end:null
    reply.songs=snapshot.rows.slice(offset,end)
  }
  const coverUri=(id,slot)=>base+'music_'+id+'_cover'+slot+'.jpg'
  async function verify(uri,m){
    let crc=0xffffffff
    for(let p=0;p<m.bytes;p+=16384){
      check();const n=Math.min(16384,m.bytes-p),r=await call('readArrayBuffer',{uri,position:p,length:n})
      if(!r.buffer||r.buffer.byteLength!==n||(r.buffer.buffer&&r.buffer.buffer.byteLength>16448))throw new Error('音乐回读长度不符')
      const b=new Uint8Array(r.buffer)
      for(let i=0;i<b.length;i++)if(Math.abs(b[i]<128?b[i]:b[i]-256)>m.peak)throw new Error('波形峰值超过声明上限 '+m.peak)
      crc=update(crc,b)
    }
    if(((crc^0xffffffff)>>>0)!==m.crc32)throw new Error('音乐 CRC32 不符')
  }
  function receipt(m){
    const r={revision:1,id:m.id,title:m.title,bytes:m.bytes,crc32:m.crc32,state:'saved',verified:true}
    // Legacy peak-64 receipts stay byte-compatible; extended transfers bind their ceiling.
    if(m.peak!==64)r.peak=m.peak
    if(m.lyrics){r.lyricsRevision=1;r.lyricsChars=m.lyrics.length}
    return r
  }
  const notifyLibrary=()=>{if(!closed)hooks.library?.(saved.map(track),{builtinRemoved})}
  function holdUnconfirmed(e){
    if(e.unconfirmed&&e.pending){libraryWriteUnconfirmed=e.pending;e.pending.then(()=>{libraryWriteUnconfirmed=null;loaded=false})}
  }
  async function writeLibrary(items,removed=[],hideBuiltin=builtinRemoved){
    await call('writeText',{uri:base+'music_library.json.tmp',text:JSON.stringify({version:2,items,removed,builtinRemoved:hideBuiltin}),append:false})
    await call('move',{srcUri:base+'music_library.json.tmp',dstUri:base+'music_library.json'})
  }
  async function cleanupDeleted(){
    const remaining=[]
    for(const id of garbage){
      let failed=false
      for(const uri of [base+'music_'+id+'.bin',coverUri(id,0),coverUri(id,1)]){
        try{await call('delete',{uri})}catch(e){if(e.code!==301)failed=true}
      }
      if(failed)remaining.push(id)
    }
    if(remaining.length!==garbage.length){await writeLibrary(saved,remaining);garbage=remaining}
  }
  async function loadLibrary(){
    let raw
    try{raw=JSON.parse((await call('readText',{uri:base+'music_library.json'})).text)}
    catch(e){if(e.code===301){saved=[];garbage=[];builtinRemoved=false;loaded=true;notifyLibrary();return}throw e}
    const items=valid(raw)?[raw]:raw&&raw.version===2?raw.items:null
    const removed=valid(raw)?[]:(raw&&raw.removed||[])
    const hideBuiltin=raw&&raw.builtinRemoved===true
    if(raw&&raw.builtinRemoved!==undefined&&typeof raw.builtinRemoved!=='boolean')throw new Error('内置试听记录无效，已保留原文件')
    if(!Array.isArray(items)||items.some(m=>!valid(m))||new Set(items.map(m=>m.id)).size!==items.length||
      !Array.isArray(removed)||removed.some(id=>!/^[a-f0-9]{32}$/.test(id)||items.some(m=>m.id===id)))throw new Error('歌曲清单无效，已保留原文件')
    for(const m of items){
      if(m.lyrics!==undefined)parseLyrics(m.lyrics)
      const info=await call('get',{uri:track(m).uri});if(info.length!==m.bytes)throw new Error('已保存歌曲长度不符')
      if(m.coverSlot!==undefined){
        if(![0,1].includes(m.coverSlot)||!Number.isInteger(m.coverBytes)||m.coverBytes<16||m.coverBytes>8192||
          !Number.isInteger(m.coverCrc32)||m.coverCrc32<0||m.coverCrc32>0xffffffff||
          !Number.isInteger(m.coverVersion)||m.coverVersion<1)throw new Error('封面记录无效，已保留原文件')
        try{
          const uri=coverUri(m.id,m.coverSlot),cover=await call('get',{uri})
          if(cover.length!==m.coverBytes)delete m.coverSlot
          else{
            const result=await call('readArrayBuffer',{uri,position:0,length:m.coverBytes})
            const bytes=new Uint8Array(result.buffer)
            if(bytes.length!==m.coverBytes||((update(0xffffffff,bytes)^0xffffffff)>>>0)!==m.coverCrc32)delete m.coverSlot
            else try{checkJpeg(bytes)}catch(_){delete m.coverSlot}
          }
        }
        catch(e){if(e.code===301)delete m.coverSlot;else throw e}
      }
    }
    check();saved=items;garbage=removed;builtinRemoved=hideBuiltin;loaded=true;notifyLibrary()
    if(garbage.length)await cleanupDeleted()
  }
  async function persistedReceipt(id){
    await loadLibrary()
    const m=saved.find(x=>x.id===id)
    if(!m)throw new Error('未找到这次传输的保存记录')
    if(current&&current.meta.id===id){current=null;disarm()}
    const pending=await readCheckpoint();if(pending&&pending.meta.id===id)await clearCheckpoint()
    publish(m,false);return m
  }
  function savedReply(m){return {ok:true,id:m.id,saved:true,next:m.bytes,receipt:receipt(m)}}
  async function cancelTransfer(id){
    let c=current
    if(!c&&!working&&id){
      try{await loadLibrary();const existing=saved.find(x=>x.id===id);if(existing){const r=await readCheckpoint();if(r&&r.meta.id===id)await clearCheckpoint();return savedReply(existing)}const r=await readCheckpoint();if(r&&r.meta.id===id){c={meta:r.meta,temp:base+'music_'+id+'.bin.tmp',resumable:true};current=c}}
      catch(e){return {ok:false,id,unconfirmed:true,error:'无法读取续传记录，请重试中断'}}
    }
    if(!c&&working&&operation==='begin'&&(!id||id===pendingId)){
      const target=pendingId;pendingCancelled=true;cancelledId=target;await done;return cancelTransfer(target)
    }
    if(!c || (id&&c.meta.id!==id)){
      const m=saved.find(x=>x.id===id);if(m)return savedReply(m)
      if(id)cancelledId=id
      return {ok:!!id,id,cancelled:!!id,error:id?'':'没有正在进行的传输'}
    }
    if(cancelling)return cancelling
    disarm()
    if(!c.publishing)c.cancelled=true
    cancelling=(async()=>{
      await done
      const m=saved.find(x=>x.id===c.meta.id)
      if(m)return savedReply(m)
      if(c.publishUnconfirmed||c.ioUnconfirmed)return {ok:false,id:c.meta.id,unconfirmed:true,error:'保存结果未确认，请查询回执'}
      // Drain the in-flight file callback before deleting any temporary output.
      try{
        for(const uri of [c.temp,c.temp+'.repair',track(c.meta).uri,...(c.resumable?[pendingUri,pendingUri+'.tmp']:[])]){try{await call('delete',{uri})}catch(e){if(e.code!==301)throw e}}
      }catch(e){return {ok:false,id:c.meta.id,unconfirmed:true,error:'传输已停止，未完成文件清理失败，请重试中断'}}
      if(current===c)current=null
      last=null;cancelledId=c.meta.id;disarm();status('传输已取消，已有歌曲保留')
      return {ok:true,id:c.meta.id,cancelled:true}
    })().finally(()=>{cancelling=null})
    return cancelling
  }
  const b64chars='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  function decodeBlock(m,c){
    if(c.transport.payloadEncoding!=='base64'){
      if(typeof m.hex!=='string'||!/^([a-f0-9]{2}){1,1024}$/.test(m.hex))throw new Error('分块格式不符')
      const b=new Uint8Array(m.hex.length/2)
      for(let i=0;i<b.length;i++)b[i]=parseInt(m.hex.slice(i*2,i*2+2),16)
      return b
    }
    const value=m.data_b64
    if(typeof value!=='string'||value.length<4||value.length>21848||value.length%4)throw new Error('Base64分块长度不符')
    const pad=value.endsWith('==')?2:value.endsWith('=')?1:0
    const b=new Uint8Array(value.length/4*3-pad)
    if(!b.length||b.length>c.transport.chunkBytes)throw new Error('分块超过协商大小')
    let out=0
    for(let i=0;i<value.length;i+=4){
      const a=b64chars.indexOf(value[i]),d=b64chars.indexOf(value[i+1])
      const e=value[i+2]==='='?0:b64chars.indexOf(value[i+2]),f=value[i+3]==='='?0:b64chars.indexOf(value[i+3])
      if(a<0||d<0||e<0||f<0||(i<value.length-4&&(value[i+2]==='='||value[i+3]==='='))||
        (value[i+2]==='='&&value[i+3]!=='=')||(i===value.length-4&&((pad===2&&(d&15))||(pad===1&&(e&3)))))throw new Error('Base64分块格式不符')
      const n=(a<<18)|(d<<12)|(e<<6)|f
      if(out<b.length)b[out++]=(n>>>16)&255
      if(out<b.length)b[out++]=(n>>>8)&255
      if(out<b.length)b[out++]=n&255
    }
    if(!Number.isInteger(m.crc32)||((update(0xffffffff,b)^0xffffffff)>>>0)!==m.crc32)throw new Error('分块 CRC32 不符')
    return b
  }
  function checkJpeg(b){
    if(b.length<16||b[0]!==255||b[1]!==216||b[b.length-2]!==255||b[b.length-1]!==217)throw new Error('封面不是完整 JPEG')
    let p=2
    while(p+7<b.length){
      if(b[p++]!==255)break
      let marker=b[p++];while(marker===255)marker=b[p++]
      if(marker===218)break
      const n=(b[p]<<8)|b[p+1]
      if(n<2||p+n>b.length)break
      if(marker===192){
        const h=(b[p+3]<<8)|b[p+4],w=(b[p+5]<<8)|b[p+6]
        if(w>=32&&w<=256&&h>=32&&h<=256&&w===h)return
        throw new Error('封面尺寸无效')
      }
      if(marker>=193&&marker<=207)throw new Error('请使用标准 JPEG 封面')
      p+=n
    }
    throw new Error('封面 JPEG 信息不完整')
  }
  const publish=(m,fresh)=>{try{hooks.imported?.(track(m),fresh)}catch(e){status('歌曲已保存，清单显示异常：'+String(e))}}
  const api={
    get busy(){return !!libraryWriteUnconfirmed||!!current||!!cancelling||(working&&operation!=='receipt')},
    get restoring(){return !!restoring},
    get phase(){return current?(operation==='commit'?'正在校验并保存歌曲':'正在接收手机音乐'):restoring?'正在读取歌曲清单':'就绪'},
    restore(){
      if(restoring)return restoring
      if(closed||working||current||cancelling||libraryWriteUnconfirmed)return Promise.resolve()
      restoring=(async()=>{
        try{await loadLibrary();if(saved.length)publish(saved[saved.length-1],false)}
        catch(e){loaded=false;holdUnconfirmed(e);if(!closed)status('读取歌曲清单失败：'+String(e.message||e))}
        finally{restoring=null}
      })()
      return restoring
    },
    cancelTransfer,
    async deleteSong(id){
      if(closed||working||current||restoring||cancelling||libraryWriteUnconfirmed||hooks.blocked?.())throw new Error('请先停止播放或等待当前操作完成')
      working=true;operation='delete';done=new Promise(resolve=>{finishWork=resolve})
      try{
        await loadLibrary()
        check()
        if(hooks.blocked?.())throw new Error('请先停止播放或等待当前操作完成')
        if(id==='/common/sample.bin'){
          if(builtinRemoved){status('内置试听已从清单删除');return}
          await writeLibrary(saved,garbage,true)
          builtinRemoved=true;generation++;notifyLibrary();status('内置试听已从清单删除')
          return
        }
        if(!saved.some(m=>m.id===id))throw new Error('歌曲不存在')
        const items=saved.filter(m=>m.id!==id),removed=garbage.concat(id)
        await writeLibrary(items,removed);saved=items;garbage=removed;last=null;generation++;notifyLibrary()
        await cleanupDeleted()
        status(garbage.length?'歌曲已移出清单，文件空间待清理':'歌曲已删除')
      }catch(e){holdUnconfirmed(e);throw e}
      finally{working=false;operation=null;finishWork?.();finishWork=null}
    },
    async handle(input){
      if(closed)return null
      let m;try{m=typeof input==='string'?JSON.parse(input):input}catch(e){return null}
      if(!m||m.tag!==tag||!/^[a-f0-9]{32}$/.test(m.id)||!Number.isInteger(m.seq)||m.seq<0)return null
      if(m.op==='cancel'){const r=await cancelTransfer(m.id);return Object.assign({tag,type:'ack',seq:m.seq,op:'cancel'},r)}
      if(working||cancelling)return null
      if(libraryWriteUnconfirmed)return {tag,type:'ack',id:m.id,seq:m.seq,op:m.op,ok:false,unconfirmed:true,error:'歌曲清单保存回调未结束，请稍后重试'}
      const signature=JSON.stringify(m)
      if(last&&last.signature===signature&&m.op!=='begin'&&m.op!=='lyrics'&&m.op!=='cover'&&m.op!=='catalog'){if(current)arm();return last.reply}
      const reply={tag,type:'ack',id:m.id,seq:m.seq,op:m.op,ok:false}
      working=true;operation=m.op;pendingId=m.op==='begin'?m.id:null;pendingCancelled=false;let cancelPending=false;done=new Promise(resolve=>{finishWork=resolve})
      try{
        if(m.op==='diagnostics'){reply.diagnostics=hooks.diagnostics?.()||{};reply.resumeRevision=1;reply.managementRevision=1
        }else if(m.op==='catalog'||m.op==='lyrics'||m.op==='cover'||m.op==='rename'||m.op==='delete'||m.op==='reorder'){
          if(current||restoring)throw new Error('歌曲操作进行中，请稍后重试')
          if(m.op!=='catalog'||!m.catalogOffset)await loadLibrary()
          if(m.op==='catalog'){
            reply.catalogRevision=1;reply.lyricsUpdateRevision=1;reply.coverRevision=1;reply.managementRevision=1;reply.moveRevision=1;reply.resumeRevision=1
            catalogPage(m,reply)
          }else if(m.op==='rename'||m.op==='delete'||m.op==='reorder'){
            if(hooks.blocked?.())throw new Error('请先停止播放，并保持手环播放器打开')
            if(m.op==='reorder'){
              if(m.moveRevision===1){
                const now=saved.map(x=>x.id),crc=orderCrc(now)
                if(crc!==m.desiredOrderCrc){
                  if(crc!==m.expectedOrderCrc||!Number.isInteger(m.moveFrom)||!Number.isInteger(m.moveTo)||
                    m.moveFrom<0||m.moveFrom>=saved.length||m.moveTo<0||m.moveTo>=saved.length||now[m.moveFrom]!==m.moveSongId)throw new Error('歌曲顺序已改变，请刷新后排序')
                  const items=saved.slice();items.splice(m.moveTo,0,items.splice(m.moveFrom,1)[0])
                  if(orderCrc(items.map(x=>x.id))!==m.desiredOrderCrc)throw new Error('目标顺序校验不符')
                  await writeLibrary(items,garbage);saved=items
                }
              }else{
              if(!Array.isArray(m.ids)||!Array.isArray(m.expectedIds)||m.ids.length!==saved.length||new Set(m.ids).size!==saved.length||m.ids.some(id=>!saved.some(x=>x.id===id)))throw new Error('歌曲清单已改变，请刷新后排序')
              const now=saved.map(x=>x.id)
              if(JSON.stringify(now)!==JSON.stringify(m.ids)){
                if(JSON.stringify(now)!==JSON.stringify(m.expectedIds))throw new Error('歌曲顺序已改变，请刷新后排序')
                const items=m.ids.map(id=>saved.find(x=>x.id===id));await writeLibrary(items,garbage);saved=items
              }
              }
            }else{
              if(!/^[a-f0-9]{32}$/.test(m.songId))throw new Error('歌曲编号无效')
              const old=saved.find(x=>x.id===m.songId)
              if(m.op==='delete'&&!old){reply.deleted=true}
              else{
                if(!old||old.bytes!==m.bytes||old.crc32!==m.crc32)throw new Error('目标歌曲已改变，请刷新手环清单')
                if(m.op==='rename'){
                  if(typeof m.title!=='string'||!m.title.trim()||m.title.length>64||/[\x00-\x1f]/.test(m.title))throw new Error('歌曲名无效')
                  if(old.title!==m.title){
                    if(old.title!==m.expectedTitle)throw new Error('歌曲名已改变，请刷新后重命名')
                    const items=saved.map(x=>x.id===old.id?Object.assign({},x,{title:m.title}):x);await writeLibrary(items,garbage);saved=items
                  }
                }else{
                  const items=saved.filter(x=>x.id!==old.id),removed=garbage.concat(old.id)
                  await writeLibrary(items,removed);saved=items;garbage=removed;await cleanupDeleted();reply.deleted=true
                }
              }
            }
            generation++;notifyLibrary();reply.managementRevision=1;reply.applied=true
            if(m.catalogPageRevision===1)reply.catalogRefresh=true;else reply.songs=catalogRows()
          }else if(m.op==='cover'){
            if(hooks.blocked?.())throw new Error('请先停止播放，并保持手环播放器打开')
            const old=saved.find(x=>x.id===m.songId)
            if(!old||old.bytes!==m.bytes||old.crc32!==m.crc32)throw new Error('目标歌曲已改变，请刷新手环清单')
            if(old.coverRequestId!==m.id){
              if(!Number.isInteger(m.coverVersion)||m.coverVersion!==(old.coverVersion||0))throw new Error('封面已更新，请刷新清单后再传')
              if(!Number.isInteger(m.coverBytes)||m.coverBytes<16||m.coverBytes>8192||typeof m.coverB64!=='string'||m.coverB64.length>10924)throw new Error('封面大小无效')
              const bytes=decodeBlock({data_b64:m.coverB64,crc32:m.coverCrc32},{transport:{payloadEncoding:'base64',chunkBytes:8192}})
              if(bytes.length!==m.coverBytes)throw new Error('封面长度不符')
              checkJpeg(bytes)
              const slot=old.coverSlot===0?1:0,uri=coverUri(old.id,slot),temp=uri+'.tmp'
              const items=saved.map(x=>x.id===old.id?Object.assign({},x,{coverSlot:slot,coverBytes:m.coverBytes,coverCrc32:m.coverCrc32,
                coverVersion:m.coverVersion+1,coverRequestId:m.id}):x)
              try{
                await call('writeArrayBuffer',{uri:temp,buffer:bytes,append:false,position:0})
                await call('move',{srcUri:temp,dstUri:uri})
                await writeLibrary(items,garbage)
              }catch(e){if(!e.unconfirmed){await remove(temp);await remove(uri)}throw e}
              saved=items;generation++;notifyLibrary();publish(items.find(x=>x.id===old.id),false);status('唱片封面已更新')
              if(Number.isInteger(old.coverSlot))await remove(coverUri(old.id,old.coverSlot))
            }else if(old.coverBytes!==m.coverBytes||old.coverCrc32!==m.coverCrc32)throw new Error('封面请求编号已使用')
            reply.coverSaved=true;reply.coverRevision=1;reply.songId=old.id;reply.bytes=old.bytes;reply.crc32=old.crc32
            reply.coverVersion=old.coverRequestId===m.id?old.coverVersion:m.coverVersion+1
            reply.coverBytes=m.coverBytes;reply.coverCrc32=m.coverCrc32
          }else{
            if(hooks.blocked?.())throw new Error('请先停止播放，并保持手环播放器打开')
            if(!/^[a-f0-9]{32}$/.test(m.songId)||typeof m.lyrics!=='string'||!m.lyrics.trim())throw new Error('歌词或目标歌曲无效')
            parseLyrics(m.lyrics)
            const old=saved.find(x=>x.id===m.songId)
            if(!old||old.bytes!==m.bytes||old.crc32!==m.crc32)throw new Error('目标歌曲已改变，请刷新手环清单')
            if(old.lyricsRequestId===m.id&&old.lyrics!==m.lyrics)throw new Error('歌词请求编号已使用')
            if(old.lyricsRequestId!==m.id){
              if(!Number.isInteger(m.lyricsVersion)||m.lyricsVersion!==(old.lyricsVersion||0))throw new Error('歌词已更新，请刷新清单后再传')
              const items=saved.map(x=>x.id===m.songId?Object.assign({},x,{lyrics:m.lyrics,lyricsRequestId:m.id,lyricsVersion:m.lyricsVersion+1}):x)
              await writeLibrary(items,garbage)
              saved=items;generation++;notifyLibrary();status('歌词已更新 · 音频保留')
            }
            reply.lyricsSaved=true;reply.lyricsUpdateRevision=1;reply.songId=old.id
            reply.bytes=old.bytes;reply.crc32=old.crc32;reply.lyricsChars=m.lyrics.length
          }
        }else if(m.op==='receipt'){
          const mSaved=await persistedReceipt(m.id)
          reply.next=mSaved.bytes;reply.saved=true;reply.receipt=receipt(mSaved)
        }else if(m.op==='commit'&&!current){
          operation='receipt'
          const mSaved=await persistedReceipt(m.id)
          if(mSaved.commitSeq!==m.seq)throw new Error('完成回执序号不符')
          reply.next=mSaved.bytes;reply.saved=true;reply.receipt=receipt(mSaved)
        }else if(m.op==='begin'){
          if(restoring)await restoring
          if(!loaded)await loadLibrary()
          if(pendingCancelled||m.id===cancelledId)throw new Error('传输已取消')
          if(current&&current.meta.id!==m.id||hooks.blocked?.())throw new Error('播放器忙，请停止后重新发送')
          if(m.seq!==0||!valid(m))throw new Error('音乐格式无效，需要 12kHz 单声道 s8、峰值1–127、最长300秒')
          if(m.lyrics!==undefined){if(typeof m.lyrics!=='string')throw new Error('歌词格式无效');parseLyrics(m.lyrics)}
          const offer=m.transport
          if(offer&&(!Number.isInteger(offer.chunkBytes)||!Number.isInteger(offer.intervalMs)||offer.chunkBytes<256||offer.chunkBytes>16384||offer.intervalMs<0||offer.intervalMs>1000))throw new Error('传输协商参数无效')
          const payloadEncoding=offer&&offer.payloadEncoding==='base64'?'base64':'hex'
          const transport={revision:1,chunkBytes:offer?Math.min(payloadEncoding==='base64'?16384:1024,offer.chunkBytes):1024,intervalMs:offer?offer.intervalMs:0,payloadEncoding}
          reply.transport=transport;reply.receiptRevision=1;reply.peakRevision=1;reply.maxPeak=127;reply.peak=m.peak;reply.libraryRevision=2;reply.songCountUnlimited=true;reply.cancelRevision=1;reply.lyricsRevision=1;reply.coverRevision=1;reply.resumeRevision=1
          const existing=saved.find(x=>x.id===m.id)
          if(existing){
            if(m.resumeRevision!==1||!sameContent(existing,m))throw new Error('传输编号已使用，请重新发送')
            reply.next=m.bytes;reply.saved=true;reply.receipt=receipt(existing);const stale=await readCheckpoint();if(stale&&stale.meta.id===m.id)await clearCheckpoint()
          }else{
            if(current?.ioUnconfirmed){const e=new Error('文件操作结果未确认，请稍后继续');e.unconfirmed=true;throw e}
            if(current&&(!sameContent(current.meta,m)||JSON.stringify(current.transport)!==JSON.stringify(transport)))throw new Error('传输内容与当前任务不符')
            if(!current){
              if(m.resumeRevision===1)current=await recover(m,transport)
              if(!current){
                // A legacy peer must not silently overwrite a recoverable task.
                if(await readCheckpoint())throw new Error('存在未完成传输，请在手机继续或中断')
                current={transport,meta:m,offset:0,seq:1,temp:base+'music_'+m.id+'.bin.tmp',resumable:m.resumeRevision===1}
                await call('writeArrayBuffer',{uri:current.temp,buffer:new Uint8Array(0),append:false,position:0})
                await checkpoint(current)
              }
            }
            generation++;check();reply.next=current.offset;reply.nextSeq=current.seq;reply.transport=current.transport
            status('接收 '+m.title+' · '+Math.floor(current.offset/m.bytes*100)+'%');arm()
          }
        }else{
          const c=current
          if(!c||c.meta.id!==m.id)throw new Error('传输会话已结束，请重新发送')
          if(c.ioUnconfirmed){const e=new Error('文件操作结果未确认，请稍后中断或查询回执');e.unconfirmed=true;throw e}
          check()
          if(m.op==='cancel'){
            current=null;disarm();await remove(c.temp);status('传输已取消')
          }else{
            if(m.seq!==c.seq)throw new Error('传输顺序不符')
            if(m.op==='chunk'){
              if(m.offset!==c.offset)throw new Error('分块位置不符')
              const bytes=decodeBlock(m,c)
              if(bytes.length>c.transport.chunkBytes||c.offset+bytes.length>c.meta.bytes||c.resumable&&bytes.length!==Math.min(c.transport.chunkBytes,c.meta.bytes-c.offset))throw new Error('分块超出声明长度')
              for(let i=0;i<bytes.length;i++)if(Math.abs(bytes[i]<128?bytes[i]:bytes[i]-256)>c.meta.peak)throw new Error('波形峰值超过声明上限 '+c.meta.peak)
              const checkpointCrc=c.transport.payloadEncoding==='base64'
                ? m.crc32
                : (update(0xffffffff,bytes)^0xffffffff)>>>0
              await checkpoint(c,{bytes:bytes.length,crc32:checkpointCrc})
              await call('writeArrayBuffer',{uri:c.temp,buffer:bytes,append:c.offset>0,position:c.offset===0?0:undefined})
              check();c.offset+=bytes.length;c.seq++
              // The pre-append intent includes this block's CRC. Recovery promotes a fully
              // written block, so another metadata rename per intermediate block is redundant.
              // Seal the final offset before commit can rename PCM and publish the library.
              if(c.offset===c.meta.bytes)await checkpoint(c)
              reply.next=c.offset;arm()
              status('接收音乐 '+Math.floor(c.offset/c.meta.bytes*100)+'%')
            }else if(m.op==='commit'){
              if(c.offset!==c.meta.bytes)throw new Error('音乐未收齐或 CRC32 不符')
              const size=await call('get',{uri:c.temp});if(size.length!==c.meta.bytes)throw new Error('音乐文件长度不符')
              status('校验手机音乐');await verify(c.temp,c.meta);check()
              c.meta=Object.assign({},c.meta,{commitSeq:m.seq})
              const t=track(c.meta)
              await call('move',{srcUri:c.temp,dstUri:t.uri});c.moved=true;check()
              const items=saved.concat(c.meta)
              await call('writeText',{uri:base+'music_library.json.tmp',text:JSON.stringify({version:2,items,removed:garbage,builtinRemoved}),append:false});check()
              c.publishing=true
              await call('move',{srcUri:base+'music_library.json.tmp',dstUri:base+'music_library.json'})
              saved=items;current=null;disarm();if(c.resumable)await clearCheckpoint();check()
              generation++;notifyLibrary();publish(c.meta,true);status('传输完成 · 已校验保存')
              reply.next=c.offset;reply.saved=true;reply.receipt=receipt(c.meta)
            }else throw new Error('未知传输操作')
          }
        }
        reply.ok=true
      }catch(e){
        if(['lyrics','cover','catalog','rename','delete','reorder'].includes(m.op))holdUnconfirmed(e)
        cancelPending=!!(current&&current.meta.id===m.id&&current.cancelled)
        if(m.id===cancelledId)reply.cancelled=true
        reply.error=String(e.message||e);reply.unconfirmed=!!e.unconfirmed;status(reply.error)
        // Abort the attempt after any uncertain append; it must never be retried.
        const c=current;if(m.op!=='receipt'&&c&&c.meta.id===m.id){
          if(e.unconfirmed){
            c.ioUnconfirmed=true
            if(e.pending)e.pending.then(()=>{
              c.ioUnconfirmed=false;loaded=false
              if(closed){if(!c.publishing&&!c.resumable){remove(c.temp);remove(track(c.meta).uri)}if(current===c)current=null}
              else if(current===c&&c.cancelled&&!c.publishing)status('文件操作已结束，请重试中断')
              else if(current===c&&c.resumable){current=null;last=null;status('文件操作已结束，可继续传输')}
            })
          }
          if(c.publishing&&e.unconfirmed)c.publishUnconfirmed=true
          if(!c.cancelled&&!c.publishUnconfirmed&&!c.ioUnconfirmed)current=null
          disarm();if(!e.unconfirmed&&!c.resumable){await remove(c.temp);if(c.moved)await remove(track(c.meta).uri)}
        }
      }finally{working=false;operation=null;pendingId=null;finishWork?.();finishWork=null}
      if(cancelPending){last=null;return null}
      if(m.op!=='receipt'&&m.op!=='begin'&&reply.ok)last={signature,reply};return reply
    },
    close(){closed=true;generation++;disarm();const c=current;if(c){c.cancelled=true;done.then(()=>{if(!c.ioUnconfirmed){if(!c.publishing&&!c.resumable){remove(c.temp);remove(track(c.meta).uri)}if(current===c)current=null}})}}
  }
  return api
}
