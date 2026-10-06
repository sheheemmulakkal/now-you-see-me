// Pure formatting helpers (no GNOME Shell imports) so they can be unit
// tested with plain gjs. Values themselves come from the collector; this
// only renders numbers with explicit units, mirroring nysm_core::units.

// Units are always explicit: binary bytes (KiB, MiB) or decimal bits
// (kb, Mb), never an ambiguous bare "M".
const BIN = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
const BITS = ['b', 'kb', 'Mb', 'Gb', 'Tb'];

function scaled(n, base, units) {
    if (!Number.isFinite(n) || n < 0)
        return null;
    let i = 0;
    while (n >= base && i < units.length - 1) {
        n /= base;
        i++;
    }
    const num = i === 0 ? `${Math.round(n)}` : n < 10 ? n.toFixed(1) : `${Math.round(n)}`;
    return [num, units[i]];
}

/** Compact binary bytes: "1.2MiB", "34KiB", "512B". */
export function compactBytes(n) {
    const v = scaled(n, 1024, BIN);
    return v ? v.join('') : '–';
}

/** Compact decimal bits from a bytes value: "9.6Mb", "270kb". */
export function compactBits(bytes) {
    const v = scaled(bytes * 8, 1000, BITS);
    return v ? v.join('') : '–';
}

/** Long form rate with explicit unit: "1.2 MiB/s" or "9.6 Mb/s". */
export function rate(bytesPerSecond, unit = 'bytes') {
    const v = unit === 'bits' ? scaled(bytesPerSecond * 8, 1000, BITS) : scaled(bytesPerSecond, 1024, BIN);
    return v ? `${v[0]} ${v[1]}/s` : '–';
}

export function percent(v, digits = 0) {
    return Number.isFinite(v) ? `${v.toFixed(digits)}%` : '–';
}

/** Read a nysm Reading<T>: the value only when status is available. */
export function live(reading) {
    return reading && reading.status === 'available' ? reading.value : null;
}

/** Pad to a fixed width so panel text does not jump as values change. */
export function pad(text, width) {
    return text.length >= width ? text : ' '.repeat(width - text.length) + text;
}
