import fs from 'node:fs';
import path from 'node:path';
const modules = new Map();
export function moduleUrl(filename) {
  if (modules.has(filename)) return modules.get(filename);
  const source = fs.readFileSync(filename, 'utf8').replace(/from '(\.\.?\/[^']+)'/g, (_, relative) =>
    'from ' + JSON.stringify(moduleUrl(path.resolve(path.dirname(filename), relative))));
  const url = 'data:text/javascript;base64,' + Buffer.from(source).toString('base64');
  modules.set(filename, url);
  return url;
}
export const drain = async () => { for (let i = 0; i < 200; i++) await Promise.resolve(); };
