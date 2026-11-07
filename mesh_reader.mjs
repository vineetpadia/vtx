// Incremental decoder for the length-prefixed JSON and binary mesh stream.
// A fragmented field gets one destination. A contiguous field stays a view.
export class MeshFrameReader {
    constructor(onFrame, reusePayload = null) {
        this.onFrame = onFrame;
        this.reusePayload = reusePayload;
        this.scratch = null;
        this.decoder = new TextDecoder('utf-8', { fatal: true });
        this.needed = 4;
        this.phase = 'length';
        this.partial = null;
        this.filled = 0;
        this.header = null;
    }

    push(chunk) {
        let offset = 0;
        while (offset < chunk.length) {
            const count = Math.min(this.needed - this.filled, chunk.length - offset);
            const fragment = chunk.subarray(offset, offset + count);
            offset += count;
            if (!this.partial && count === this.needed) {
                this.accept(fragment);
                continue;
            }
            if (!this.partial) {
                // Opt in only for payloads consumed synchronously by onFrame.
                if (this.phase === 'payload' && this.needed <= 1024 * 1024 &&
                    this.reusePayload?.(this.header)) {
                    if (!this.scratch || this.scratch.length < this.needed) {
                        this.scratch = new Uint8Array(this.needed);
                    }
                    this.partial = this.scratch.subarray(0, this.needed);
                } else {
                    this.partial = new Uint8Array(this.needed);
                }
            }
            this.partial.set(fragment, this.filled);
            this.filled += count;
            if (this.filled === this.needed) {
                const field = this.partial;
                this.partial = null;
                this.filled = 0;
                this.accept(field);
            }
        }
    }

    accept(field) {
        if (this.phase === 'length') {
            const size = new DataView(field.buffer, field.byteOffset, 4).getUint32(0, true);
            if (!size || size > 1024 * 1024) throw Error('Invalid mesh frame header size');
            this.needed = size;
            this.phase = 'header';
        } else if (this.phase === 'header') {
            this.header = JSON.parse(this.decoder.decode(field));
            const size = this.header.payload_bytes;
            if (!Number.isSafeInteger(size) || size < 0 || size > 0x7fffffff) {
                throw Error('Invalid mesh frame payload size');
            }
            if (size) {
                this.needed = size;
                this.phase = 'payload';
            } else {
                this.emit(null);
            }
        } else {
            this.emit(field);
        }
    }

    emit(payload) {
        const header = this.header;
        this.header = null;
        this.phase = 'length';
        this.needed = 4;
        this.onFrame(header, payload);
    }

    finish() {
        if (this.phase !== 'length' || this.filled) throw Error('Incomplete mesh stream');
    }
}

export function words(payload, Type) {
    const width = Type.BYTES_PER_ELEMENT;
    if (!payload || payload.byteLength % width) throw Error('Invalid mesh payload alignment');
    if (payload.byteOffset % width) {
        const aligned = new Uint8Array(payload.byteLength);
        aligned.set(payload);
        payload = aligned;
    }
    return new Type(payload.buffer, payload.byteOffset, payload.byteLength / width);
}
