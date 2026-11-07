// Texture frames contain original encoded images. Keep image decode off the
// synchronous mesh receive path and share the result between primitives.
export class TextureBank {
    constructor(gl, redraw, beforeUpload = null) {
        this.gl = gl;
        this.redraw = redraw;
        this.beforeUpload = beforeUpload;
        this.textures = new Map();
        this.ids = new Set();
        this.pending = null;
        this.tasks = [];
        this.active = true;
        this.failed = 0;
    }

    receive(header, payload) {
        if (header.type === 'texture') {
            const { id, size, mime } = header;
            if (this.pending || this.ids.has(id) || !Number.isSafeInteger(id) ||
                id < 0 || !Number.isSafeInteger(size) || size <= 0 ||
                size > 512 * 1024 * 1024 || !['image/png', 'image/jpeg'].includes(mime)) {
                throw Error('Invalid texture header');
            }
            this.ids.add(id);
            this.pending = { header, chunks: [], length: 0 };
            return;
        }
        const image = this.pending;
        if (!image || !payload?.length || image.length + payload.length > image.header.size) {
            throw Error('Invalid texture data');
        }
        image.chunks.push(payload);
        image.length += payload.length;
        if (image.length === image.header.size) {
            this.pending = null;
            const blob = new Blob(image.chunks, { type: image.header.mime });
            this.tasks.push(this.upload(image.header, blob).catch(() => { this.failed++; }));
        }
    }

    async upload(header, blob) {
        const gl = this.gl;
        const image = await createImageBitmap(blob, {
            imageOrientation: 'none', premultiplyAlpha: 'none', colorSpaceConversion: 'none'
        });
        let texture;
        try {
            if (!this.active) return;
            this.beforeUpload?.();
            const max = gl.getParameter(gl.MAX_TEXTURE_SIZE);
            if (image.width > max || image.height > max) throw Error('Texture exceeds WebGL limits');
            texture = gl.createTexture();
            if (!texture) throw Error('Unable to allocate texture');
            gl.bindTexture(gl.TEXTURE_2D, texture);
            gl.texImage2D(gl.TEXTURE_2D, 0, gl.SRGB8_ALPHA8, gl.RGBA, gl.UNSIGNED_BYTE, image);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, header.wrap_s);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, header.wrap_t);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, header.min);
            gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, header.mag);
            if (header.min >= gl.NEAREST_MIPMAP_NEAREST) gl.generateMipmap(gl.TEXTURE_2D);
            if (gl.getError() !== gl.NO_ERROR) throw Error('Unable to upload texture');
            this.textures.set(header.id, texture);
            texture = null;
            this.redraw();
        } finally {
            if (texture) gl.deleteTexture(texture);
            image.close();
        }
    }

    async finish() {
        if (this.pending) throw Error('Incomplete texture stream');
        await Promise.all(this.tasks);
        this.tasks.length = 0;
    }

    dispose() {
        this.active = false;
        for (const texture of this.textures.values()) this.gl.deleteTexture(texture);
        this.textures.clear();
        this.pending = null;
    }
}
