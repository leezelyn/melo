import {execFileSync} from 'node:child_process';
import {readFile,mkdir,copyFile,writeFile,rm} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {join,dirname} from 'node:path';
import {createHash} from 'node:crypto';
const root=join(dirname(fileURLToPath(import.meta.url)),'..'),dist=join(root,'dist');
const manifest=JSON.parse(await readFile(join(root,'manifest.json'),'utf8'));
execFileSync('cargo',['build','--release','--target','wasm32-wasip2','--locked'],{cwd:root,stdio:'inherit'});
await mkdir(dist,{recursive:true});
await copyFile(join(root,'target/wasm32-wasip2/release/huanjian_music_plugin.wasm'),join(dist,manifest.entry));
for(const name of ['manifest.json',manifest.icon])await copyFile(join(root,name),join(dist,name));
const bytes=await readFile(join(dist,manifest.entry));
await writeFile(join(dist,'build-info.json'),JSON.stringify({version:manifest.version,apiLevel:3,wasiVersion:2,world:'psys-world-v3',language:'Rust',witBindgen:'0.46.0',runtimeAbi:'Wasmtime 38.0.4',runtimeDispatch:'V3 synchronous exports with callback-scoped block_on',sha256:createHash('sha256').update(bytes).digest('hex')},null,2)+'\n');
console.log('Built API Level 3 / WASI 2: '+bytes.length+' bytes');
if(process.argv.includes('--package')){
 execFileSync(process.execPath,['scripts/check-component.mjs'],{cwd:root,stdio:'inherit'});
 execFileSync(process.execPath,['scripts/check-runtime.mjs'],{cwd:root,stdio:'inherit'});
 const abp=join(dist,'HuanjianMusic-AstroBox-'+manifest.version+'.abp');await rm(abp,{force:true});
 execFileSync('zip',['-q',abp,'manifest.json',manifest.entry,manifest.icon],{cwd:dist});console.log('Packaged '+abp);
}
