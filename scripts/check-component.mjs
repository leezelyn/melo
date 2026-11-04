// Inspect the actual compiled ABI. User retains ABox and real-device acceptance.
import {execFileSync} from 'node:child_process';
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const wit=execFileSync('node_modules/.bin/jco',['wit','dist/plugin.wasm'],{encoding:'utf8'});
assert.match(wit,/import astrobox:psys-host\/ui-v3;/);
assert.match(wit,/export astrobox:psys-plugin\/event-v3;/);
assert.doesNotMatch(wit,/psys-(?:host|plugin)-v4|http-server|\/browser[;@]/);
const manifest=JSON.parse(readFileSync('dist/manifest.json'));
assert.equal(manifest.api_level,3);assert.equal(manifest.wasi_version,2);
for(const permission of ['device','thirdpartyapp','interconnect','register_interconnect_recv']){
 assert.ok(manifest.permissions.includes(permission),'Missing V3 operation permission: '+permission);
}
assert.ok(!manifest.permissions.includes('browser')&&!manifest.permissions.includes('http-server'));
console.log('Compiled ABI PASS: API Level 3, WASI 2, native UI + event-v3; no V4 / Browser / HTTP interfaces');
