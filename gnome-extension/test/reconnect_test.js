// Starts with no collector; expects 'disconnected', then a connection once
// a collector appears. Usage: the harness starts `nysm service run` ~3 s in.
import GLib from 'gi://GLib';
import {CollectorClient} from '../nysm@nysm.dev/client.js';

const loop = new GLib.MainLoop(null, false);
const states = [];
const client = new CollectorClient({
    onState: s => states.push(s),
    onUpdate: () => {
        client.stop();
        loop.quit();
    },
});
client.start();
GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 15, () => {
    client.stop();
    loop.quit();
    return GLib.SOURCE_REMOVE;
});
loop.run();
print(`states=${states.join(',')}`);
const ok = states.includes('disconnected') && states.at(-1) === 'connected';
imports.system.exit(ok ? 0 : 1);
