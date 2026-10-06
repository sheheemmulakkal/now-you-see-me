// Now You See Me panel indicator for GNOME Shell 46.
//
// Presentation only: it subscribes to the per-user collector service
// (`nysm service run`) with a lite subscription and renders what it
// receives. No metrics are collected inside GNOME Shell. Every signal,
// timer and socket is released in disable().

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import St from 'gi://St';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import {CollectorClient, socketPath} from './client.js';
import {compactBits, compactBytes, live, pad, percent, rate} from './format.js';

const HISTORY = 60; // samples kept for popover sparklines
const STALE_FACTOR = 3;

const Sparkline = GObject.registerClass(
class NysmSparkline extends St.DrawingArea {
    _init(color) {
        super._init({style_class: 'nysm-sparkline', width: 90, height: 18});
        this._values = [];
        this._max = 100;
        this._color = color;
        this.connect('repaint', () => this._paint());
    }

    setValues(values, max) {
        this._values = values;
        this._max = max;
        this.queue_repaint();
    }

    _paint() {
        const cr = this.get_context();
        const [w, h] = this.get_surface_size();
        const n = this._values.length;
        if (n >= 2) {
            cr.setSourceRGBA(...this._color, 1);
            cr.setLineWidth(1.2);
            let pen = false;
            this._values.forEach((v, i) => {
                const x = (w * i) / (n - 1);
                if (v === null || v === undefined) {
                    pen = false; // gaps are not drawn across
                    return;
                }
                const y = h - 1 - (h - 2) * Math.min(Math.max(v / this._max, 0), 1);
                if (pen)
                    cr.lineTo(x, y);
                else
                    cr.moveTo(x, y);
                pen = true;
            });
            cr.stroke();
        }
        cr.$dispose();
    }
});

const Indicator = GObject.registerClass(
class NysmIndicator extends PanelMenu.Button {
    _init(extension) {
        super._init(0.0, 'Now You See Me');
        this._ext = extension;
        this._settings = extension.getSettings();
        this._history = [];
        this._intervalMs = 1000;
        this._lastUpdate = 0;
        this._state = 'connecting';

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box nysm-panel'});
        this._cpu = new St.Label({style_class: 'nysm-value', y_align: Clutter.ActorAlign.CENTER});
        this._mem = new St.Label({style_class: 'nysm-value', y_align: Clutter.ActorAlign.CENTER});
        this._net = new St.Label({style_class: 'nysm-value nysm-net', y_align: Clutter.ActorAlign.CENTER});
        box.add_child(this._cpu);
        box.add_child(this._mem);
        box.add_child(this._net);
        this.add_child(box);

        this._rows = {};
        const section = new PopupMenu.PopupMenuSection();
        for (const [key, title, color] of [
            ['cpu', 'CPU', [0.11, 0.53, 0.87]],
            ['mem', 'Memory', [0.57, 0.34, 0.80]],
            ['swap', 'Swap', null],
            ['rx', 'Network ↓', [0.18, 0.66, 0.40]],
            ['tx', 'Network ↑', [0.11, 0.45, 0.30]],
            ['disk', 'Disk r / w', null],
            ['load', 'Load', null],
            ['psi', 'Pressure cpu/mem/io', null],
        ]) {
            const item = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
            const name = new St.Label({text: title, style_class: 'nysm-row-name'});
            const value = new St.Label({text: '–', style_class: 'nysm-row-value', x_expand: true, x_align: Clutter.ActorAlign.END});
            item.add_child(name);
            item.add_child(value);
            let spark = null;
            if (color) {
                spark = new Sparkline(color);
                item.add_child(spark);
            }
            section.addMenuItem(item);
            this._rows[key] = {value, spark};
        }
        this.menu.addMenuItem(section);
        this._status = new PopupMenu.PopupMenuItem('', {reactive: false, can_focus: false});
        this._status.label.add_style_class_name('nysm-status');
        this.menu.addMenuItem(this._status);
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        const open = new PopupMenu.PopupMenuItem('Open monitor');
        open.connect('activate', () => this._openMonitor());
        this.menu.addMenuItem(open);

        this._settingsIds = ['show-cpu', 'show-memory', 'show-network', 'compact', 'rate-unit'].map(k =>
            this._settings.connect(`changed::${k}`, () => this._render()));

        this._client = new CollectorClient({
            onState: (state, detail) => {
                this._state = state;
                this._detail = detail;
                this._render();
            },
            onWelcome: msg => {
                this._intervalMs = msg.interval_ms || 1000;
                this._history = (msg.history || []).slice(-HISTORY);
            },
            onUpdate: msg => {
                this._snapshot = msg.snapshot;
                this._history.push(...(msg.history || []));
                if (this._history.length > HISTORY)
                    this._history.splice(0, this._history.length - HISTORY);
                this._alerts = (msg.alerts || []).filter(a => a.state === 'firing');
                this._lastUpdate = GLib.get_monotonic_time();
                this._render();
            },
        });
        this._client.start();

        // Staleness check: values must never look live when they are not.
        this._staleId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT_IDLE, 2, () => {
            this._render();
            return GLib.SOURCE_CONTINUE;
        });
        this._render();
    }

    _isStale() {
        if (!this._lastUpdate)
            return true;
        const ageMs = (GLib.get_monotonic_time() - this._lastUpdate) / 1000;
        return ageMs > this._intervalMs * STALE_FACTOR + 1000;
    }

    _render() {
        const compact = this._settings.get_boolean('compact');
        const bits = this._settings.get_string('rate-unit') === 'bits';
        const s = this._snapshot;
        const connected = this._state === 'connected';
        const stale = !connected || this._isStale();

        this._cpu.visible = this._settings.get_boolean('show-cpu');
        this._mem.visible = this._settings.get_boolean('show-memory');
        this._net.visible = this._settings.get_boolean('show-network');

        if (!connected || !s) {
            this._cpu.text = compact ? '–' : 'nysm –';
            this._mem.text = '';
            this._net.text = '';
            this._mem.visible = this._net.visible = false;
        } else {
            const cpu = live(s.cpu.usage);
            const mem = live(s.memory.usage);
            const net = live(s.network.total);
            const cpuText = pad(cpu ? percent(cpu.total_pct) : '–', 4);
            const memText = pad(mem ? percent(mem.used_pct) : '–', 4);
            const fmt = bits ? compactBits : compactBytes;
            const netText = net ? `↓${pad(fmt(net.rx_bytes_per_s), 6)} ↑${pad(fmt(net.tx_bytes_per_s), 6)}` : '↓ – ↑ –';
            this._cpu.text = compact ? cpuText : `CPU ${cpuText}`;
            this._mem.text = compact ? memText : `MEM ${memText}`;
            this._net.text = netText;
        }
        for (const l of [this._cpu, this._mem, this._net]) {
            if (stale)
                l.add_style_class_name('nysm-stale');
            else
                l.remove_style_class_name('nysm-stale');
        }
        if (this._alerts?.length)
            this.add_style_class_name('nysm-alerting');
        else
            this.remove_style_class_name('nysm-alerting');

        this._renderMenu(s, bits);
    }

    _renderMenu(s, bits) {
        const r = this._rows;
        const unit = bits ? 'bits' : 'bytes';
        const set = (k, t) => {
            r[k].value.text = t;
        };
        if (s) {
            const cpu = live(s.cpu.usage);
            const mem = live(s.memory.usage);
            const swap = live(s.memory.swap);
            const net = live(s.network.total);
            const disk = live(s.storage.total_io);
            const load = live(s.cpu.load);
            const cores = live(s.cpu.logical_cores);
            const psi = p => {
                const v = live(p);
                return v && v.some.interval_pct !== undefined ? percent(v.some.interval_pct, 1) : '–';
            };
            set('cpu', cpu ? percent(cpu.total_pct, 1) : '–');
            set('mem', mem ? `${compactBytes(mem.used_bytes)} / ${compactBytes(mem.total_bytes)} (${percent(mem.used_pct)})` : '–');
            set('swap', swap ? (swap.total_bytes ? `${compactBytes(swap.used_bytes)} / ${compactBytes(swap.total_bytes)}` : 'none') : '–');
            set('rx', net ? rate(net.rx_bytes_per_s, unit) : '–');
            set('tx', net ? rate(net.tx_bytes_per_s, unit) : '–');
            set('disk', disk ? `${rate(disk.read_bytes_per_s)} / ${rate(disk.write_bytes_per_s)}` : '–');
            set('load', load ? `${load.one.toFixed(2)} on ${cores ?? '?'} cores` : '–');
            set('psi', `${psi(s.cpu.pressure)} / ${psi(s.memory.pressure)} / ${psi(s.storage.io_pressure)}`);
        }
        const h = this._history;
        const series = f => h.map(p => (p.gap_before ? null : f(p)));
        r.cpu.spark.setValues(series(p => p.cpu_pct), 100);
        r.mem.spark.setValues(series(p => p.mem_used_pct), 100);
        const rx = series(p => p.net_rx_bytes_per_s);
        const tx = series(p => p.net_tx_bytes_per_s);
        const netMax = Math.max(1, ...rx.filter(v => v !== null), ...tx.filter(v => v !== null));
        r.rx.spark.setValues(rx, netMax);
        r.tx.spark.setValues(tx, netMax);

        let status;
        if (this._state === 'connected')
            status = this._isStale() ? 'Collector not responding — values are stale' : 'Live · totals cover physical interfaces and whole disks';
        else if (this._state === 'connecting')
            status = 'Connecting to collector…';
        else
            status = `Collector not running (start: nysm service run) — ${this._detail ?? ''}`;
        if (this._alerts?.length)
            status = `${this._alerts.length} alert(s) firing · ${status}`;
        this._status.label.text = status;
    }

    _openMonitor() {
        const app = Gio.DesktopAppInfo.new('dev.nysm.NowYouSeeMe.desktop');
        try {
            if (app)
                app.launch([], null);
            else
                GLib.spawn_command_line_async('nysm-desktop');
        } catch (e) {
            Main.notifyError('Now You See Me', `Could not start the monitor: ${e.message}`);
        }
    }

    destroy() {
        this._client?.stop();
        this._client = null;
        if (this._staleId) {
            GLib.source_remove(this._staleId);
            this._staleId = 0;
        }
        for (const id of this._settingsIds ?? [])
            this._settings.disconnect(id);
        this._settingsIds = [];
        this._settings = null;
        super.destroy();
    }
});

export default class NysmExtension extends Extension {
    enable() {
        this._indicator = new Indicator(this);
        Main.panel.addToStatusArea(this.uuid, this._indicator);
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;
    }
}

// Exported for diagnostics only.
export {socketPath};
