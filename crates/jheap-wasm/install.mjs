// Builds jheap's parser as WebAssembly and copies it into mountain-graph's
// page, which has no build step of its own: the module is committed there.
//
//   node crates/jheap-wasm/install.mjs [<mountain-graph dir>]
//
// The directory is mountain-graph beside this checkout by default, or the one
// MOUNTAIN_GRAPH names.
import {execFileSync} from 'node:child_process';
import {copyFileSync, existsSync, statSync} from 'node:fs';
import {dirname, join, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const page = resolve(process.argv[2] ?? process.env.MOUNTAIN_GRAPH ?? join(root, '..', 'mountain-graph'));
const target = join(page, 'web', 'js', 'hprof', 'jheap.wasm');
if (!existsSync(dirname(target))) {
  console.error(`${page} is not mountain-graph: no web/js/hprof in it`);
  process.exit(2);
}

execFileSync('cargo', ['build', '-p', 'jheap-wasm', '--target', 'wasm32-unknown-unknown', '--release'],
  {cwd: root, stdio: 'inherit'});
const built = join(root, 'target', 'wasm32-unknown-unknown', 'release', 'jheap_wasm.wasm');
copyFileSync(built, target);
const commit = execFileSync('git', ['rev-parse', '--short', 'HEAD'], {cwd: root, encoding: 'utf8'}).trim();
const dirty = execFileSync('git', ['status', '--porcelain'], {cwd: root, encoding: 'utf8'}).trim() !== '';
console.log(`${target}: ${(statSync(target).size / 1024).toFixed(0)} KiB, jheap ${commit}${dirty ? ' with changes not committed' : ''}`);
