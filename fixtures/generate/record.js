// @ts-check
/**
 * Builds the test dumps, then records what heap-landscape's JS implementation
 * makes of them:
 *
 * - `expected/<dump>.parse.json`, the parse: every path from a root with its
 *   bytes and objects, and the counts of the parse report. jheap's parser must
 *   give the same, since the web app draws exactly that.
 * - `baseline/`, what the `hl` command printed. Only a baseline to compare
 *   with: jheap's command line is free to say it better.
 *
 * Run from anywhere, with heap-landscape checked out beside jheap or named by
 * HEAP_LANDSCAPE:
 *
 *     node fixtures/generate/record.js
 */
import {execFile} from 'node:child_process';
import {mkdir, readFile, rm, writeFile} from 'node:fs/promises';
import {dirname, join, resolve} from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {promisify} from 'node:util';
import {HprofBuilder, TYPE} from './hprof-builder.js';

const run = promisify(execFile);
const fixtures = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const landscape = resolve(process.env.HEAP_LANDSCAPE ?? join(fixtures, '..', '..', 'heap-landscape'));
const hl = join(landscape, 'tools', 'report.mjs');
const {parseHprof} = await import(pathToFileURL(join(landscape, 'web', 'js', 'hprof', 'parser.js')).href);

/**
 * A heap described by class names and fields, over HprofBuilder: ids are
 * handed out in order, and an instance's fields are written in the order HPROF
 * keeps them, its class's first, then its super class's.
 */
class Heap {
  /** @param {number} idSize @param {number} base the address of the first class or object */
  constructor(idSize, base) {
    this.b = new HprofBuilder({idSize});
    this.address = base;
    this.stringId = 0x10;
    this.serial = 1;
    /** @type {Map<string, number>} */
    this.strings = new Map();
    /** @type {Map<string, {id: number, fields: [string, number][], sup: string | null}>} */
    this.classes = new Map();
  }

  /** @returns {number} the next address, objects 32 bytes apart */
  next() {
    const id = this.address;
    this.address += 0x20;
    return id;
  }

  /** @param {string} s */
  str(s) {
    let id = this.strings.get(s);
    if (id === undefined) {
      id = this.stringId++;
      this.strings.set(s, id);
      this.b.string(id, s);
    }
    return id;
  }

  /**
   * @param {string} name as LOAD_CLASS writes it: `com/example/Server`, `[Ljava/lang/Object;`
   * @param {[string, number][]} [fields] name and type, of this class only
   * @param {string | null} [sup]
   * @param {[string, number][]} [statics] name and the object it holds
   */
  cls(name, fields = [], sup = null, statics = []) {
    const id = this.next();
    this.b.loadClass(id, this.str(name), this.serial++);
    this.classes.set(name, {id, fields, sup});
    let size = 0;
    for (let c = this.classes.get(name); c; c = c.sup ? this.classes.get(c.sup) : undefined)
      size += c.fields.reduce((s, [, t]) => s + (t === TYPE.OBJECT ? this.b.idSize : 4), 0);
    this.b.classDump({
      classId: id,
      superId: sup ? this.classes.get(sup)?.id : 0,
      instanceSize: size,
      fields: fields.map(([n, type]) => ({nameId: this.str(n), type})),
      staticFields: statics.map(([n, value]) => ({nameId: this.str(n), type: TYPE.OBJECT, value})),
    });
    return id;
  }

  /**
   * @param {string} name
   * @param {Record<string, number>} values by field name, 0 for null
   * @param {number} [id]
   */
  obj(name, values = {}, id = this.next()) {
    /** @type {Uint8Array[]} */
    const bytes = [];
    for (let c = this.classes.get(name); c; c = c.sup ? this.classes.get(c.sup) : undefined)
      for (const [field, type] of c.fields)
        bytes.push(type === TYPE.OBJECT ? this.b.id(values[field] ?? 0) : this.b.u32(values[field] ?? 0));
    this.b.instance(id, /** @type {number} */ (this.classes.get(name)?.id), concat(bytes));
    return id;
  }

  /** @param {number} length @param {number} [id] */
  bytes(length, id = this.next()) {
    this.b.primitiveArray(id, TYPE.BYTE, length, new Uint8Array(length));
    return id;
  }

  /** @param {number} length @param {number} [id] */
  ints(length, id = this.next()) {
    this.b.primitiveArray(id, TYPE.INT, length, new Uint8Array(length * 4));
    return id;
  }

  /** @param {string} arrayClass @param {number[]} elements @param {number} [id] */
  array(arrayClass, elements, id = this.next()) {
    this.b.objectArray(id, /** @type {number} */ (this.classes.get(arrayClass)?.id), elements);
    return id;
  }
}

/** @param {Uint8Array[]} parts */
function concat(parts) {
  const out = new Uint8Array(parts.reduce((s, p) => s + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** A class whose static field holds an object, rooted as a sticky class: 4-byte ids. */
function staticHolder() {
  const h = new Heap(4, 0x1000);
  const held = h.next();
  const holder = h.cls('Holder', [], null, [['held', held]]);
  h.cls('Held');
  h.obj('Held', {}, held);
  h.b.rootStickyClass(holder);
  return h.b.build();
}

/** B extends A: an instance holds B's fields, then A's; 4-byte ids. */
function inheritedFields() {
  const h = new Heap(4, 0x1000);
  h.cls('A', [['a', TYPE.OBJECT]]);
  h.cls('B', [['n', TYPE.INT], ['b', TYPE.OBJECT]], 'A');
  h.cls('TargetA');
  h.cls('TargetB');
  const a = h.obj('TargetA');
  const b = h.obj('TargetB');
  h.b.rootJavaFrame(h.obj('B', {n: 42, b, a}));
  return h.b.build();
}

/**
 * A server, as a 64-bit JVM dumps it: a registry of sessions that grows with
 * `sessions`, a cache over a HashMap, a JDK class holding a buffer in a static,
 * a long chain of links, objects nothing reaches, a reference to an object
 * defined further on, and one to an object never defined.
 *
 * @param {number} sessions
 */
function server(sessions) {
  const h = new Heap(8, 0x7_0000_0000);
  h.cls('java/lang/Object');
  h.cls('[Ljava/lang/Object;');
  h.cls('java/lang/String', [['value', TYPE.OBJECT], ['hash', TYPE.INT]], 'java/lang/Object');
  h.cls('java/util/HashMap', [['table', TYPE.OBJECT], ['size', TYPE.INT]], 'java/lang/Object');
  h.cls('java/util/HashMap$Node', [['key', TYPE.OBJECT], ['value', TYPE.OBJECT], ['next', TYPE.OBJECT]],
        'java/lang/Object');
  h.cls('[Ljava/util/HashMap$Node;');
  const buffer = h.next();
  const source = h.cls('java/util/zip/ZipFile$Source', [], 'java/lang/Object', [['buffer', buffer]]);
  h.cls('com/example/Server', [['registry', TYPE.OBJECT], ['cache', TYPE.OBJECT], ['missing', TYPE.OBJECT],
        ['port', TYPE.INT]], 'java/lang/Object');
  h.cls('com/example/Registry', [['sessions', TYPE.OBJECT]], 'java/lang/Object');
  h.cls('com/example/BaseSession', [['id', TYPE.INT], ['user', TYPE.OBJECT]], 'java/lang/Object');
  h.cls('com/example/Session', [['data', TYPE.OBJECT]], 'com/example/BaseSession');
  h.cls('com/example/Cache', [['map', TYPE.OBJECT]], 'java/lang/Object');
  h.cls('com/example/Chain$Link', [['next', TYPE.OBJECT], ['payload', TYPE.OBJECT]], 'java/lang/Object');

  h.bytes(64 * 1024, buffer);
  h.b.rootStickyClass(source);

  /** @param {string} s */
  const string = s => h.obj('java/lang/String', {value: h.bytes(s.length), hash: s.length});

  // the registry: sessions, a null among them
  const users = Array.from({length: 5}, (_, i) => string(`user-${i}`));
  const list = [];
  for (let i = 0; i < sessions; i++) {
    list.push(h.obj('com/example/Session', {id: i, user: users[i % users.length], data: h.bytes(1000 + (i % 7) * 100)}));
    if (i === 3)
      list.push(0);
  }
  const registry = h.obj('com/example/Registry', {sessions: h.array('[Ljava/lang/Object;', list)});

  // the cache: a HashMap of 20 entries, some chained in a bucket
  const nodes = [];
  for (let i = 0; i < 20; i++)
    nodes.push(h.obj('java/util/HashMap$Node', {key: string(`key-${i}`), value: h.ints(50 + i * 10),
                                                next: i % 4 === 1 ? nodes[i - 1] : 0}));
  const table = h.array('[Ljava/util/HashMap$Node;', nodes.filter((_, i) => i % 4 !== 0));
  const cache = h.obj('com/example/Cache', {map: h.obj('java/util/HashMap', {table, size: 20})});

  // the server: its registry is defined before it, its cache after (below), and `missing` never
  const later = h.next();
  const app = h.obj('com/example/Server', {registry, cache: later, missing: 0x7_FFFF_0000, port: 8080});
  h.obj('com/example/Cache', {map: 0}, later);
  h.b.rootJavaFrame(app);
  h.b.rootJavaFrame(cache);

  // a chain of 40 links, from a root of unknown kind
  let link = 0;
  for (let i = 0; i < 40; i++)
    link = h.obj('com/example/Chain$Link', {next: link, payload: i % 10 === 0 ? h.bytes(512) : 0});
  h.b.rootUnknown(link);

  // what nothing reaches
  for (let i = 0; i < 5; i++)
    h.bytes(2000);
  h.obj('com/example/Session', {id: -1, data: h.bytes(3000)});
  return h.b.build();
}

/** @type {Record<string, Uint8Array>} */
const dumps = {
  'static-holder': staticHolder(),
  'inherited-fields': inheritedFields(),
  'server-before': server(50),
  'server-after': server(400),
};
// a dump cut short, as a disk that filled up leaves it
dumps['truncated'] = dumps['server-after'].subarray(0, Math.floor(dumps['server-after'].length * 0.6));

for (const sub of ['dumps', 'expected', 'baseline']) {
  await rm(join(fixtures, sub), {recursive: true, force: true});
  await mkdir(join(fixtures, sub), {recursive: true});
}
for (const [name, bytes] of Object.entries(dumps))
  await writeFile(join(fixtures, 'dumps', `${name}.hprof`), bytes);

// --- the parse, which jheap must give exactly ---

/** The report's fields that describe the dump, not the run: no times, no file name. */
const TIMES = new Set(['fileName', 'parseTimeMs', 'metadataTimeMs', 'objectsTimeMs', 'graphTimeMs']);

/** @param {string} name @param {'auto' | 'compressed' | 'wide'} pointers */
async function parse(name, pointers) {
  const bytes = await readFile(join(fixtures, 'dumps', `${name}.hprof`));
  try {
    const result = await parseHprof(new File([bytes], `${name}.hprof`), () => {}, {pointers});
    const paths = [...result.stacks.keys()].sort().map(path =>
      ({path: path.split(';'), bytes: result.stacks.get(path), objects: result.objects.get(path)}));
    const report = Object.fromEntries(Object.entries(result.report).filter(([k]) => !TIMES.has(k)));
    return {dump: `${name}.hprof`, pointers, report, paths};
  }
  catch (e) {
    return {dump: `${name}.hprof`, pointers, error: /** @type {Error} */ (e).message};
  }
}

const parses = [
  ...Object.keys(dumps).map(name => [name, 'auto']),
  ['server-after', 'wide'],
];
for (const [name, pointers] of parses) {
  const suffix = pointers === 'auto' ? '' : `.${pointers}`;
  const doc = await parse(name, /** @type {'auto' | 'wide'} */ (pointers));
  await writeFile(join(fixtures, 'expected', `${name}${suffix}.parse.json`), JSON.stringify(doc, null, 1) + '\n');
}

// --- what hl printed: a baseline, not a target ---

/** @type {[string, string[]][]} case name, arguments */
const cases = [
  ['usage', []],
  ['truncated', ['truncated.hprof']],
  ['diff', ['diff', 'server-before.hprof', 'server-after.hprof']],
  ['diff.json', ['diff', 'server-before.hprof', 'server-after.hprof', '--json']],
];
for (const name of ['static-holder', 'inherited-fields', 'server-after'])
  for (const command of [[], ['peaks'], ['leaks']])
    for (const json of [[], ['--json']])
      cases.push([[name, ...command, ...json.map(() => 'json')].join('.'), [...command, `${name}.hprof`, ...json]]);
cases.push(['server-after.trace', ['server-after.hprof', '--trace', 'com.example.Session']]);

const manifest = [];
for (const [name, args] of cases) {
  let code = 0, stdout = '', stderr = '';
  try {
    ({stdout, stderr} = await run(process.execPath, [hl, ...args, '--no-keep'], {cwd: join(fixtures, 'dumps')}));
  }
  catch (e) {
    const failed = /** @type {{code?: number, stdout?: string, stderr?: string}} */ (e);
    ({code = 1, stdout = '', stderr = ''} = failed);
  }
  const out = `${name}.${name.endsWith('.json') ? 'out.json' : 'out.txt'}`.replace('.json.out.json', '.out.json');
  await writeFile(join(fixtures, 'baseline', out), stdout);
  manifest.push({name, args: ['hl', ...args], exit: code, stdout: out, stderr: stderr.split('\n').filter(Boolean)});
}
await writeFile(join(fixtures, 'baseline', 'cases.json'), JSON.stringify(manifest, null, 1) + '\n');

console.log(`${Object.keys(dumps).length} dumps, ${parses.length} parses, ${cases.length} baseline cases, in ${fixtures}`);
