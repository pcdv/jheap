// @ts-check
/**
 * Builds small HPROF files for the tests: a header, strings, class loads, and
 * heap dump segments with the sub-records the parser reads.
 */

/** @param {...Uint8Array} parts */
export function concat(...parts) {
  return concatAll(parts);
}

/**
 * As concat, from an array: a dump of many records is more than a call can take as arguments.
 *
 * @param {Uint8Array[]} parts
 */
function concatAll(parts) {
  const total = parts.reduce((s, p) => s + p.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export class HprofBuilder {
  /** @param {{idSize?: number}} [options] */
  constructor({idSize = 4} = {}) {
    this.idSize = idSize;
    /** @type {Uint8Array[]} */
    this.chunks = [];
    /** @type {Uint8Array[]} */
    this.segment = [];
  }

  /** @param {number} n */
  id(n) {
    const out = new Uint8Array(this.idSize);
    const v = new DataView(out.buffer);
    if (this.idSize === 4)
      v.setUint32(0, n, false);
    else
      v.setBigUint64(0, BigInt(n), false);
    return out;
  }

  /** @param {number} n */
  u32(n) {
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, n, false);
    return out;
  }

  /** @param {number} n */
  u16(n) {
    const out = new Uint8Array(2);
    new DataView(out.buffer).setUint16(0, n, false);
    return out;
  }

  /** @param {number} n */
  u8(n) {
    return new Uint8Array([n]);
  }

  /** @param {number} tag @param {Uint8Array} body */
  record(tag, body) {
    const out = new Uint8Array(9 + body.length);
    const v = new DataView(out.buffer);
    out[0] = tag;
    v.setUint32(5, body.length, false);
    out.set(body, 9);
    return out;
  }

  /** @param {number} id @param {string} name */
  string(id, name) {
    this.chunks.push(this.record(0x01, concat(this.id(id), new TextEncoder().encode(name))));
    return this;
  }

  /** @param {number} classId @param {number} nameId @param {number} [serial] */
  loadClass(classId, nameId, serial = 1) {
    this.chunks.push(this.record(0x02, concat(this.u32(serial), this.id(classId), this.u32(0), this.id(nameId))));
    return this;
  }

  /**
   * @param {object} c
   * @param {number} c.classId
   * @param {number} [c.superId]
   * @param {number} [c.instanceSize]
   * @param {{nameId: number, type: number, value: number}[]} [c.staticFields]
   * @param {{type: number, value: number}[]} [c.constantPool]
   * @param {{nameId: number, type: number}[]} [c.fields]
   * @param {number} [c.loaderId]
   * @param {number} [c.signersId]
   * @param {number} [c.protectionDomainId]
   */
  classDump({classId, superId = 0, instanceSize = 0, staticFields = [], constantPool = [], fields = [],
             loaderId = 0, signersId = 0, protectionDomainId = 0}) {
    const statics = staticFields.flatMap(f => [this.id(f.nameId), this.u8(f.type), this.value(f.type, f.value)]);
    const constants = constantPool.flatMap((c, i) => [this.u16(i + 1), this.u8(c.type), this.value(c.type, c.value)]);
    const instances = fields.flatMap(f => [this.id(f.nameId), this.u8(f.type)]);
    this.segment.push(concat(
      this.u8(0x20),
      this.id(classId), this.u32(0), this.id(superId),
      this.id(loaderId), this.id(signersId), this.id(protectionDomainId), this.id(0), this.id(0),
      this.u32(instanceSize),
      this.u16(constantPool.length), ...constants,
      this.u16(staticFields.length), ...statics,
      this.u16(fields.length), ...instances,
    ));
    return this;
  }

  /**
   * @param {number} type
   * @param {number} value an object id for TYPE_OBJECT, the number otherwise
   */
  value(type, value) {
    if (type === 2)
      return this.id(value);
    if (type === 11)
      return concat(this.u32(0), this.u32(value));
    if (type === 7)
      return concat(this.u32(0), this.u32(value));
    if (type === 4 || type === 8)
      return this.u8(value);
    if (type === 5 || type === 9)
      return this.u16(value);
    return this.u32(value);
  }

  /** @param {number} id @param {number} classId @param {Uint8Array} [bytes] */
  instance(id, classId, bytes = new Uint8Array(0)) {
    this.segment.push(concat(this.u8(0x21), this.id(id), this.u32(0), this.id(classId), this.u32(bytes.length), bytes));
    return this;
  }

  /** @param {number} id @param {number} arrayClassId @param {number[]} elements */
  objectArray(id, arrayClassId, elements) {
    this.segment.push(concat(
      this.u8(0x22), this.id(id), this.u32(0), this.u32(elements.length), this.id(arrayClassId),
      ...elements.map(e => this.id(e)),
    ));
    return this;
  }

  /** @param {number} id @param {number} elemType @param {number} count @param {Uint8Array} data */
  primitiveArray(id, elemType, count, data) {
    this.segment.push(concat(
      this.u8(0x23), this.id(id), this.u32(0), this.u32(count),
      this.u8(elemType), data,
    ));
    return this;
  }

  /** @param {number} id */
  rootUnknown(id) {
    this.segment.push(concat(this.u8(0xff), this.id(id)));
    return this;
  }

  /** @param {number} classId */
  rootStickyClass(classId) {
    this.segment.push(concat(this.u8(0x05), this.id(classId)));
    return this;
  }

  /** @param {number} id @param {number} [threadSerial] @param {number} [frame] */
  rootJavaFrame(id, threadSerial = 1, frame = 0) {
    this.segment.push(concat(this.u8(0x03), this.id(id), this.u32(threadSerial), this.u32(frame)));
    return this;
  }

  /** @returns {Uint8Array} */
  build() {
    const version = new TextEncoder().encode('JAVA PROFILE 1.0.2');
    const header = new Uint8Array(version.length + 1 + 4 + 8);
    header.set(version, 0);
    header[version.length] = 0;
    new DataView(header.buffer).setUint32(version.length + 1, this.idSize, false);
    const segment = this.segment.length > 0 ? [this.record(0x1c, concatAll(this.segment))] : [];
    return concatAll([header, ...this.chunks, ...segment]);
  }

  /** @param {string} [name] */
  file(name = 'test.hprof') {
    return new File([this.build()], name);
  }
}

/** Field type constants, as the parser reads them. */
export const TYPE = {OBJECT: 2, BOOLEAN: 4, CHAR: 5, FLOAT: 6, DOUBLE: 7, BYTE: 8, SHORT: 9, INT: 10, LONG: 11};
