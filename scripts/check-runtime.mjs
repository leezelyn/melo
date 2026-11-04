// Actual WASM callback regression, using local host adapters only.
import {execFileSync} from 'node:child_process';
import {dirname,join} from 'node:path';
const wasm=process.argv[2]||'dist/plugin.wasm';
for(const [format,recovery] of [['payloadText','normal'],['raw','normal'],['payloadText','legacy-blocked']]){
 execFileSync('cargo',['run','--locked','--manifest-path','tools/runtime38/Cargo.toml','--',wasm,join(dirname(wasm),'manifest.json'),format,recovery],{stdio:'inherit'});
}
