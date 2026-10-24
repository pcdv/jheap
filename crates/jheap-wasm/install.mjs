// Builds jheap's parser as WebAssembly and copies it into heap-landscape's
// page, which has no build step of its own: the module is committed there.
//
//   node crates/jheap-wasm/install.mjs [<heap-landscape dir>]
//
// The directory is heap-landscape beside this checkout by default, or the one
// HEAP_LANDSCAPE names.
import {execFileSync} from 'node:child_process';
import {copyFileSync, existsSync, statSync} from 'node:fs';
import {dirname, join, resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const landscape = resolve(process.argv[2] ?? process.env.HEAP_LANDSCAPE ?? join(root, '..', 'heap-landscape'));
const target = join(landscape, 'web', 'js', 'hprof', 'jheap.wasm');
if (!existsSync(dirname(target))) {
  console.error(`${landscape} is not heap-landscape: no web/js/hprof in it`);
  process.exit(2);
}

execFileSync('cargo', ['build', '-p', 'jheap-wasm', '--target', 'wasm32-unknown-unknown', '--release'],
  {cwd: root, stdio: 'inherit'});
const built = join(root, 'target', 'wasm32-unknown-unknown', 'release', 'jheap_wasm.wasm');
copyFileSync(built, target);
const commit = execFileSync('git', ['rev-parse', '--short', 'HEAD'], {cwd: root, encoding: 'utf8'}).trim();
const dirty = execFileSync('git', ['status', '--porcelain'], {cwd: root, encoding: 'utf8'}).trim() !== '';
console.log(`${target}: ${(statSync(target).size / 1024).toFixed(0)} KiB, jheap ${commit}${dirty ? ' with changes not committed' : ''}`);
