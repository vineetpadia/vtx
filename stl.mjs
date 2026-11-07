// Returns packed float32 triangle positions in file order.
export function readStl(bytes) {
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (view.byteLength >= 84) {
        const triangles = view.getUint32(80, true);
        if (84 + triangles * 50 === view.byteLength) {
            if (!triangles) throw Error('STL has no triangles');
            const positions = new Float32Array(triangles * 9);
            const words = new Uint32Array(positions.buffer);
            for (let src = 96, dst = 0; dst < words.length; src += 50, dst += 9) {
                // Fixed-width records let the engine eliminate the inner loop.
                const a = view.getUint32(src, true);
                const b = view.getUint32(src + 4, true);
                const c = view.getUint32(src + 8, true);
                const d = view.getUint32(src + 12, true);
                const e = view.getUint32(src + 16, true);
                const f = view.getUint32(src + 20, true);
                const g = view.getUint32(src + 24, true);
                const h = view.getUint32(src + 28, true);
                const i = view.getUint32(src + 32, true);
                if (!finite(a) || !finite(b) || !finite(c) || !finite(d) || !finite(e) ||
                    !finite(f) || !finite(g) || !finite(h) || !finite(i)) {
                    throw Error('STL contains a non-finite vertex');
                }
                words[dst] = a;
                words[dst + 1] = b;
                words[dst + 2] = c;
                words[dst + 3] = d;
                words[dst + 4] = e;
                words[dst + 5] = f;
                words[dst + 6] = g;
                words[dst + 7] = h;
                words[dst + 8] = i;
            }
            return positions;
        }
    }
    return readAsciiStl(new TextDecoder('utf-8', {fatal: true}).decode(bytes));
}

function finite(word) {
    return (word & 0x7f800000) !== 0x7f800000;
}

function readAsciiStl(text) {
    const token = /\S+/g;
    const next = () => token.exec(text)?.[0];
    function expect(word) {
        if (next() !== word) throw Error('Malformed ASCII STL: expected ' + word);
    }
    function coordinate() {
        const value = next();
        const number = Math.fround(Number(value));
        if (!/^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i.test(value) || !Number.isFinite(number)) {
            throw Error('Invalid ASCII STL coordinate');
        }
        return number;
    }
    function skipName() {
        const newline = text.indexOf('\n', token.lastIndex);
        token.lastIndex = newline === -1 ? text.length : newline + 1;
    }
    let positions = new Float32Array(9 * 1024);
    let length = 0;
    let keyword = next();
    while (keyword === 'solid') {
        skipName();
        while ((keyword = next()) === 'facet') {
            expect('normal');
            for (let i = 0; i < 3; i++) coordinate();
            expect('outer');
            expect('loop');
            if (length + 9 > positions.length) {
                const larger = new Float32Array(positions.length * 2);
                larger.set(positions);
                positions = larger;
            }
            for (let vertex = 0; vertex < 3; vertex++) {
                expect('vertex');
                for (let axis = 0; axis < 3; axis++) positions[length++] = coordinate();
            }
            expect('endloop');
            expect('endfacet');
        }
        if (keyword !== 'endsolid') throw Error('Malformed ASCII STL: expected endsolid');
        skipName();
        keyword = next();
    }
    if (keyword !== undefined || !length) throw Error('Invalid or empty STL');
    return positions.subarray(0, length);
}
