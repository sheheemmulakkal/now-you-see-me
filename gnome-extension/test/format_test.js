// gjs -m gnome-extension/test/format_test.js
import {compactBits, compactBytes, live, pad, percent, rate} from '../nysm@nysm.dev/format.js';

let failures = 0;
function eq(actual, expected) {
    if (actual !== expected) {
        failures++;
        print(`FAIL: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
}
eq(compactBytes(0), '0B');
eq(compactBytes(1536), '1.5KiB');
eq(compactBytes(15 * 1024 * 1024), '15MiB');
eq(compactBytes(NaN), '–');
eq(compactBytes(-1), '–');
eq(compactBits(125000), '1.0Mb');
eq(compactBits(12), '96b');
eq(rate(1024), '1.0 KiB/s');
eq(rate(500), '500 B/s');
eq(rate(125000, 'bits'), '1.0 Mb/s');
eq(rate(undefined), '–');
eq(percent(12.34), '12%');
eq(percent(12.34, 1), '12.3%');
eq(percent(undefined), '–');
eq(live({status: 'available', value: 3}), 3);
eq(live({status: 'warming_up'}), null);
eq(pad('5%', 4), '  5%');
print(failures ? `${failures} failures` : 'format: all passed');
if (failures)
    imports.system.exit(1);
