// WFW Protocol Client — implements the wfw framing protocol for file transfer
// This is a minimal JS implementation of wfw-client functionality
const WFW = {
  CHUNK_SIZE: 256 * 1024, // 256 KiB
  MAX_RETRIES: 5,

  // CRC32 implementation (table-driven)
  _crcTable: null,
  _initCrcTable() {
    if (this._crcTable) return;
    this._crcTable = new Uint32Array(256);
    for (let i = 0; i < 256; i++) {
      let crc = i;
      for (let j = 0; j < 8; j++) {
        crc = crc & 1 ? (crc >>> 1) ^ 0xEDB88320 : crc >>> 1;
      }
      this._crcTable[i] = crc;
    }
  },

  crc32(data) {
    this._initCrcTable();
    let crc = 0xFFFFFFFF;
    for (let i = 0; i < data.length; i++) {
      crc = this._crcTable[(crc ^ data[i]) & 0xFF] ^ (crc >>> 8);
    }
    return (crc ^ 0xFFFFFFFF) >>> 0;
  },

  // Build a wfw frame
  buildFrame(type_, path, offset, end, payload) {
    const MAGIC = new Uint8Array([0x57, 0x46, 0x57, 0x00]); // "WFW\0"
    const version = 1;
    const metaLen = 0;
    const pathBytes = new TextEncoder().encode(path);
    const payloadLen = payload ? payload.length : 0;

    let fullPayload;
    if (payload) {
      fullPayload = new Uint8Array(payloadLen + 4);
      fullPayload.set(payload);
    } else {
      fullPayload = new Uint8Array(4); // just CRC32 placeholder
    }

    const header = new ArrayBuffer(40 + pathBytes.length);
    const dv = new DataView(header);
    let pos = 0;

    MAGIC.forEach(b => dv.setUint8(pos++, b));
    dv.setUint8(pos++, version);
    dv.setUint8(pos++, type_);
    dv.setUint32(pos, metaLen, true); pos += 4;
    dv.setUint32(pos, pathBytes.length, true); pos += 4;
    dv.setBigUint64(pos, BigInt(offset), true); pos += 8;
    dv.setBigUint64(pos, BigInt(end), true); pos += 8;
    dv.setUint32(pos, 0, true); pos += 4; // CRC32 placeholder in header too

    // Write path
    new Uint8Array(header).set(pathBytes, 40);

    // Compute CRC32 over header + path + payload
    const headerBytes = new Uint8Array(header);
    const crcInput = new Uint8Array(headerBytes.length + payloadLen);
    crcInput.set(headerBytes);
    if (payload) crcInput.set(payload, headerBytes.length);
    const crc = this.crc32(crcInput);

    // Update CRC in the payload area (last 4 bytes)
    if (payload) {
      fullPayload[payloadLen] = crc & 0xFF;
      fullPayload[payloadLen + 1] = (crc >> 8) & 0xFF;
      fullPayload[payloadLen + 2] = (crc >> 16) & 0xFF;
      fullPayload[payloadLen + 3] = (crc >> 24) & 0xFF;
    }

    return { header: new Uint8Array(header), fullPayload };
  },

  // Upload a file using wfw protocol
  async uploadFile(file, uploadPath, token, _wfwPort, onProgress) {
    const totalSize = file.size;
    let uploaded = 0;
    let retries = 0;

    const uploadChunk = async (startOffset) => {
      const end = Math.min(startOffset + this.CHUNK_SIZE, totalSize);
      const chunk = await file.slice(startOffset, end).arrayBuffer();
      const { header, fullPayload } = this.buildFrame(1, uploadPath, startOffset, end, new Uint8Array(chunk));

      // Combine header + fullPayload for the request body
      const body = new Uint8Array(header.length + fullPayload.length);
      body.set(header);
      body.set(fullPayload, header.length);

      const res = await fetch(`/wfw/upload`, {
        method: 'PUT',
        headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/octet-stream' },
        body,
      });

      if (!res.ok) {
        if (retries < this.MAX_RETRIES) {
          retries++;
          await new Promise(r => setTimeout(r, 1000 * retries));
          return uploadChunk(startOffset);
        }
        throw new Error(`Upload failed at offset ${startOffset}: HTTP ${res.status}`);
      }

      retries = 0;
      uploaded = end;
      if (onProgress) onProgress(uploaded, totalSize);

      if (end < totalSize) {
        return uploadChunk(end);
      }
    };

    await uploadChunk(0);
    return true;
  },

  // Download a file using wfw protocol
  async downloadFile(downloadPath, token, _wfwPort, onProgress) {
    const res = await fetch(`/wfw/download?path=${encodeURIComponent(downloadPath)}`, {
      method: 'GET',
      headers: { 'Authorization': `Bearer ${token}` },
    });

    if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);

    const reader = res.body.getReader();
    const chunks = [];
    let totalBytes = 0;

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      totalBytes += value.length;
      if (onProgress) onProgress(totalBytes);
    }

    // Combine chunks
    const result = new Uint8Array(totalBytes);
    let offset = 0;
    for (const chunk of chunks) {
      result.set(chunk, offset);
      offset += chunk.length;
    }

    return result;
  },

  // Simple direct upload (fallback for non-wfw)
  async simpleUpload(file, token, _wfwPort, onProgress) {
    return new Promise((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open('PUT', `/wfw/upload`, true);
      xhr.setRequestHeader('Authorization', `Bearer ${token}`);
      xhr.setRequestHeader('Content-Type', 'application/octet-stream');

      xhr.upload.onprogress = (e) => {
        if (e.lengthComputable && onProgress) onProgress(e.loaded, e.total);
      };

      xhr.onload = () => {
        if (xhr.status >= 200 && xhr.status < 300) resolve(true);
        else reject(new Error(`Upload failed: HTTP ${xhr.status}`));
      };

      xhr.onerror = () => reject(new Error('Upload network error'));
      xhr.send(file);
    });
  },

  // Simple direct download
  async simpleDownload(downloadPath, token, _wfwPort) {
    const res = await fetch(`/wfw/download?path=${encodeURIComponent(downloadPath)}`, {
      headers: { 'Authorization': `Bearer ${token}` },
    });
    if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);
    const blob = await res.blob();
    return blob;
  },
};
