// Bounded LRC; no file reads or timers on the playback path.
export const MAX_LRC_CHARS=4096
export function parseLyrics(raw){
  if(typeof raw!=='string'||raw.length>MAX_LRC_CHARS)throw new Error('歌词需为 UTF-8 LRC，最多 4096 字符')
  const rows=[];let offset=0,order=0
  const lines=raw.replace(/^\uFEFF/,'').split(/\r\n|\n|\r/)
  for(const line of lines){
    const meta=/^\[offset:([+-]?\d+)\]\s*$/i.exec(line)
    if(meta){offset=Number(meta[1]);if(!Number.isFinite(offset)||Math.abs(offset)>60000)throw new Error('歌词 offset 超过 60 秒');continue}
    const stamps=[];let rest=line,match
    while((match=/^\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]/.exec(rest))){
      if(Number(match[2])>=60)throw new Error('歌词秒数无效')
      stamps.push(Number(match[1])*60000+Number(match[2])*1000+Number((match[3]||'').padEnd(3,'0')))
      rest=rest.slice(match[0].length)
    }
    const text=rest.replace(/[\x00-\x1f\u2028\u2029]/g,' ').trim()
    if(text.length>160&&stamps.length)throw new Error('单句歌词超过 160 字符')
    for(const ms of stamps){rows.push({ms,text,order:order++});if(rows.length>256)throw new Error('歌词最多 256 个时间点')}
  }
  if(raw.trim()&&!rows.some(r=>r.text))throw new Error('未找到带时间戳的 LRC 歌词')
  // LRC offset adjusts lyric timestamps; positive values advance the lyric.
  rows.forEach(r=>{r.ms=Math.max(0,r.ms-offset)})
  rows.sort((a,b)=>a.ms-b.ms||a.order-b.order)
  const merged=[]
  rows.forEach(r=>{const last=merged[merged.length-1];if(last&&last.ms===r.ms){if(r.text)last.text=last.text?last.text+' / '+r.text:r.text}else merged.push({ms:r.ms,text:r.text})})
  return merged
}
export function lyricIndex(rows,ms){
  let low=0,high=rows.length-1,result=-1
  while(low<=high){const mid=(low+high)>>1;if(rows[mid].ms<=ms){result=mid;low=mid+1}else high=mid-1}
  return result
}
