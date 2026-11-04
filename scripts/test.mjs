import {execFileSync} from 'node:child_process';
execFileSync('cargo',['test','--lib','--locked'],{stdio:'inherit'});
execFileSync('cargo',['build','--example','protocol_bridge','--locked'],{stdio:'inherit'});
execFileSync(process.execPath,['--test','tests/receiver.test.mjs'],{stdio:'inherit'});
