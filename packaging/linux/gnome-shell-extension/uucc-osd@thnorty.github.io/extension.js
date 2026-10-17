// Shows Unofficial Uniwill Control Center's pop-ups as GNOME's own. GNOME Shell keeps its pop-up call (ShowOSD) to its
// own components, so the app asks this extension instead, on the session bus:
//   io.github.thnorty.uucc.Osd  /io/github/thnorty/uucc/Osd  Show(icon, label, level)
// `icon` is an icon name, `label` the text ("" for none), `level` the bar's fraction (negative for no bar).

import Gio from 'gi://Gio';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'io.github.thnorty.uucc.Osd';
const OBJECT_PATH = '/io/github/thnorty/uucc/Osd';
const INTERFACE = `
<node>
  <interface name="io.github.thnorty.uucc.Osd">
    <method name="Show">
      <arg type="s" name="icon" direction="in"/>
      <arg type="s" name="label" direction="in"/>
      <arg type="d" name="level" direction="in"/>
    </method>
  </interface>
</node>`;

export default class UuccOsdExtension extends Extension {
    enable() {
        this._object = Gio.DBusExportedObject.wrapJSObject(INTERFACE, this);
        this._object.export(Gio.DBus.session, OBJECT_PATH);
        this._name = Gio.bus_own_name_on_connection(Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    disable() {
        Gio.bus_unown_name(this._name);
        this._object.unexport();
        this._object = null;
    }

    Show(icon, label, level) {
        const gicon = Gio.ThemedIcon.new_with_default_fallbacks(icon);
        Main.osdWindowManager.showAll(gicon, label === '' ? null : label, level < 0 ? null : level, 1);
    }
}
