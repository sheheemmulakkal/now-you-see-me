// Collector client: Gio-only (works in gjs and in GNOME Shell). Speaks the
// nysm IPC protocol v1: 4-byte big-endian length + JSON frames over a
// user-only Unix socket. No collection happens here.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

Gio._promisify(Gio.SocketClient.prototype, 'connect_async');
Gio._promisify(Gio.InputStream.prototype, 'read_bytes_async');
Gio._promisify(Gio.OutputStream.prototype, 'write_all_async');

export const PROTOCOL_VERSION = 1;
const MAX_FRAME = 8 * 1024 * 1024;

export function socketPath() {
    const override = GLib.getenv('NYSM_RUNTIME_DIR');
    if (override)
        return GLib.build_filenamev([override, 'collector.sock']);
    const runtime = GLib.getenv('XDG_RUNTIME_DIR');
    if (runtime)
        return GLib.build_filenamev([runtime, 'nysm', 'collector.sock']);
    return GLib.build_filenamev([GLib.get_tmp_dir(), `nysm-${new Gio.Credentials().get_unix_user()}`, 'collector.sock']);
}

function encodeFrame(obj) {
    const body = new TextEncoder().encode(JSON.stringify(obj));
    const frame = new Uint8Array(4 + body.length);
    new DataView(frame.buffer).setUint32(0, body.length, false);
    frame.set(body, 4);
    return frame;
}

/**
 * Connects, reconnects with backoff, and reports:
 *   onState('connecting' | 'connected' | 'disconnected', detail)
 *   onWelcome(msg), onUpdate(msg)
 */
export class CollectorClient {
    constructor({onState, onWelcome, onUpdate, client = 'nysm-gnome-extension', lite = true}) {
        this._onState = onState ?? (() => {});
        this._onWelcome = onWelcome ?? (() => {});
        this._onUpdate = onUpdate ?? (() => {});
        this._client = client;
        this._lite = lite;
        this._cancellable = null;
        this._conn = null;
        this._retryId = 0;
        this._backoff = 2;
        this._running = false;
    }

    start() {
        if (this._running)
            return;
        this._running = true;
        this._connect();
    }

    stop() {
        this._running = false;
        if (this._retryId) {
            GLib.source_remove(this._retryId);
            this._retryId = 0;
        }
        this._cancellable?.cancel();
        this._cancellable = null;
        this._closeConnection();
    }

    _closeConnection() {
        try {
            this._conn?.close(null);
        } catch (e) {
            // Already closed.
        }
        this._conn = null;
    }

    _scheduleRetry(detail) {
        this._closeConnection();
        if (!this._running)
            return;
        this._onState('disconnected', detail);
        const delay = this._backoff;
        this._backoff = Math.min(this._backoff * 2, 30);
        this._retryId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, delay, () => {
            this._retryId = 0;
            this._connect();
            return GLib.SOURCE_REMOVE;
        });
    }

    async _readExactly(stream, n) {
        const out = new Uint8Array(n);
        let got = 0;
        while (got < n) {
            const bytes = await stream.read_bytes_async(n - got, GLib.PRIORITY_DEFAULT, this._cancellable);
            const chunk = bytes.toArray();
            if (chunk.length === 0)
                throw new Error('collector closed the connection');
            out.set(chunk, got);
            got += chunk.length;
        }
        return out;
    }

    async _connect() {
        if (!this._running)
            return;
        this._cancellable = new Gio.Cancellable();
        this._onState('connecting', socketPath());
        try {
            const client = new Gio.SocketClient();
            const address = Gio.UnixSocketAddress.new(socketPath());
            this._conn = await client.connect_async(address, this._cancellable);
            const output = this._conn.get_output_stream();
            const input = this._conn.get_input_stream();
            const hello = {
                type: 'hello',
                protocol_version: PROTOCOL_VERSION,
                client: this._client,
                subscribe: {processes: false, lite: this._lite},
            };
            await output.write_all_async(encodeFrame(hello), GLib.PRIORITY_DEFAULT, this._cancellable);
            const decoder = new TextDecoder();
            for (;;) {
                const head = await this._readExactly(input, 4);
                const len = new DataView(head.buffer).getUint32(0, false);
                if (len > MAX_FRAME)
                    throw new Error(`frame of ${len} bytes exceeds limit`);
                const msg = JSON.parse(decoder.decode(await this._readExactly(input, len)));
                if (msg.type === 'welcome') {
                    this._backoff = 2;
                    this._onState('connected', msg.producer);
                    this._onWelcome(msg);
                } else if (msg.type === 'update') {
                    this._onUpdate(msg);
                } else if (msg.type === 'error') {
                    throw new Error(msg.message);
                }
            }
        } catch (e) {
            if (e.matches?.(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED) || !this._running)
                return;
            this._scheduleRetry(e.message);
        }
    }
}
