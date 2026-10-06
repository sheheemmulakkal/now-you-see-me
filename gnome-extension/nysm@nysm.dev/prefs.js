import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import Gtk from 'gi://Gtk';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

export default class NysmPrefs extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();
        const page = new Adw.PreferencesPage();
        const group = new Adw.PreferencesGroup({
            title: 'Panel',
            description: 'Values come from the nysm collector service (nysm service run).',
        });
        for (const [key, title] of [
            ['show-cpu', 'Show CPU'],
            ['show-memory', 'Show memory'],
            ['show-network', 'Show network receive/transmit'],
            ['compact', 'Compact (values only, no labels)'],
        ]) {
            const row = new Adw.SwitchRow({title});
            settings.bind(key, row, 'active', Gio.SettingsBindFlags.DEFAULT);
            group.add(row);
        }
        const unit = new Adw.ComboRow({title: 'Network unit', model: Gtk.StringList.new(['bytes/s', 'bits/s'])});
        unit.selected = settings.get_string('rate-unit') === 'bits' ? 1 : 0;
        unit.connect('notify::selected', () => settings.set_string('rate-unit', unit.selected === 1 ? 'bits' : 'bytes'));
        group.add(unit);
        page.add(group);
        window.add(page);
    }
}
