// Connects to a running collector with the extension's client and checks
// that welcome + lite updates arrive. Usage (collector must be running):
//   NYSM_RUNTIME_DIR=/tmp/x gjs -m gnome-extension/test/client_test.js
import GLib from 'gi://GLib';
import {CollectorClient, socketPath} from '../nysm@nysm.dev/client.js';

const loop = new GLib.MainLoop(null, false);
let updates = 0;
let welcomed = false;
const states = [];
const client = new CollectorClient({
    onState: s => states.push(s),
    onWelcome: () => {
        welcomed = true;
    },
    onUpdate: msg => {
        updates++;
        const s = msg.snapshot;
        const lite = s.cpu.per_core.length === 0 && s.network.interfaces.value === undefined;
        print(`update seq=${msg.seq} cpu=${JSON.stringify(s.cpu.usage.value?.total_pct)} lite=${lite} history=${msg.history.length}`);
        if (updates >= 3) {
            client.stop();
            loop.quit();
        }
    },
});
print(`socket: ${socketPath()}`);
client.start();
GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 10, () => {
    print('timeout');
    client.stop();
    loop.quit();
    return GLib.SOURCE_REMOVE;
});
loop.run();
print(`states=${states.join(',')} welcomed=${welcomed} updates=${updates}`);
imports.system.exit(welcomed && updates >= 3 ? 0 : 1);
