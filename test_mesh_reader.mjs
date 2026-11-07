import assert from 'node:assert/strict';
import { MeshFrameReader, words } from './mesh_reader.mjs';

function frame(type, payload = Buffer.alloc(0)) {
    const header = Buffer.from(JSON.stringify({ type, payload_bytes: payload.length }));
    const length = Buffer.alloc(4);
    length.writeUInt32LE(header.length);
    return Buffer.concat([length, header, payload]);
}

const metadata = frame('meta');
const indices = frame('indices', Buffer.from([11, 0, 0, 0, 17, 0, 0, 0]));
const stream = Buffer.concat([metadata, indices, frame('done')]);
for (let step = 1; step <= stream.length; step++) {
    const events = [];
    const reader = new MeshFrameReader((event, payload) => {
        events.push([event.type, payload ? Array.from(words(payload, Uint32Array)) : null]);
    });
    for (let offset = 0; offset < stream.length; offset += step) {
        reader.push(stream.subarray(offset, offset + step));
    }
    reader.finish();
    assert.deepEqual(events, [['meta', null], ['indices', [11, 17]], ['done', null]]);
}

for (let stop = 1; stop < stream.length; stop++) {
    if (stop === metadata.length || stop === metadata.length + indices.length) continue;
    const reader = new MeshFrameReader(() => {});
    reader.push(stream.subarray(0, stop));
    assert.throws(() => reader.finish(), /Incomplete mesh stream/);
}

assert.throws(() => new MeshFrameReader(() => {}).push(Buffer.alloc(4)), /Invalid mesh frame/);

const reused = [];
const retained = [];
const copies = [];
const reader = new MeshFrameReader((event, payload) => {
    if (event.type === 'indices') {
        reused.push(payload.buffer);
        copies.push(Array.from(payload));
    } else {
        retained.push(payload);
    }
}, event => event.type === 'indices');
const fragmented = Buffer.concat([
    frame('indices', Buffer.from([1, 2, 3, 4])),
    frame('texture_bytes', Buffer.from([9, 8, 7, 6])),
    frame('indices', Buffer.from([4, 3, 2])),
    frame('texture_bytes', Buffer.from([5, 6, 7])),
]);
for (let offset = 0; offset < fragmented.length; offset++) {
    reader.push(fragmented.subarray(offset, offset + 1));
}
reader.finish();
assert.equal(reused[0], reused[1]);
assert.deepEqual(copies, [[1, 2, 3, 4], [4, 3, 2]]);
assert.deepEqual(retained.map(bytes => Array.from(bytes)), [[9, 8, 7, 6], [5, 6, 7]]);
