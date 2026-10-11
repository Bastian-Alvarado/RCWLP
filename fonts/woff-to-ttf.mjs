// Converts a WOFF (version 1) font to TTF, which Android and Windows can load.
//   node woff-to-ttf.mjs input.woff output.ttf
// WOFF is the TTF's tables, each zlib-compressed, behind a different header: this inflates the
// tables and writes them back behind a TTF header, unchanged otherwise.
import fs from 'node:fs';
import zlib from 'node:zlib';

const [input, output] = process.argv.slice(2);
const woff = fs.readFileSync(input);
if (woff.toString('latin1', 0, 4) !== 'wOFF') throw new Error(`${input} is not a WOFF 1 font`);
const flavor = woff.readUInt32BE(4);
const count = woff.readUInt16BE(12);

const tables = [];
for (let i = 0; i < count; i++) {
  const at = 44 + i * 20;
  const tag = woff.toString('latin1', at, at + 4);
  const offset = woff.readUInt32BE(at + 4);
  const compressed = woff.readUInt32BE(at + 8);
  const length = woff.readUInt32BE(at + 12);
  const checksum = woff.readUInt32BE(at + 16);
  const raw = woff.subarray(offset, offset + compressed);
  const data = compressed < length ? zlib.inflateSync(raw) : raw;
  if (data.length !== length) throw new Error(`table ${tag}: expected ${length} bytes, got ${data.length}`);
  tables.push({ tag, checksum, data });
}
tables.sort((a, b) => (a.tag < b.tag ? -1 : 1));

const pad4 = (n) => (n + 3) & ~3;
const power = 2 ** Math.floor(Math.log2(count));
const header = Buffer.alloc(12 + 16 * count);
header.writeUInt32BE(flavor, 0);
header.writeUInt16BE(count, 4);
header.writeUInt16BE(power * 16, 6);
header.writeUInt16BE(Math.log2(power), 8);
header.writeUInt16BE(count * 16 - power * 16, 10);
let offset = header.length;
const bodies = tables.map((t, i) => {
  const at = 12 + i * 16;
  header.write(t.tag, at, 'latin1');
  header.writeUInt32BE(t.checksum, at + 4);
  header.writeUInt32BE(offset, at + 8);
  header.writeUInt32BE(t.data.length, at + 12);
  const body = Buffer.alloc(pad4(t.data.length));
  t.data.copy(body);
  offset += body.length;
  return body;
});
fs.writeFileSync(output, Buffer.concat([header, ...bodies]));
console.log(`${output}: ${count} tables, ${offset} bytes`);
