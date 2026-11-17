import pathlib
import sys
import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName('org.kde.StatusNotifierWatcher', bus=bus)
marker = pathlib.Path(sys.argv[1])

class Watcher(dbus.service.Object):
    def __init__(self):
        super().__init__(bus, '/StatusNotifierWatcher')
        self.items = []

    @dbus.service.method('org.freedesktop.DBus.Introspectable', in_signature='', out_signature='s')
    def Introspect(self):
        return '''<node><interface name="org.kde.StatusNotifierWatcher">
        <method name="RegisterStatusNotifierItem"><arg direction="in" type="s"/></method>
        <method name="RegisterStatusNotifierHost"><arg direction="in" type="s"/></method>
        <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
        <signal name="StatusNotifierHostRegistered"/>
        <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
        <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
        <property name="ProtocolVersion" type="i" access="read"/>
        </interface></node>'''

    @dbus.service.method('org.kde.StatusNotifierWatcher', in_signature='s', out_signature='', sender_keyword='sender')
    def RegisterStatusNotifierItem(self, item, sender=None):
        address = str(sender) + str(item) if str(item).startswith('/') else str(item) + '/StatusNotifierItem'
        self.items.append(address)
        marker.write_text(address)
        self.StatusNotifierItemRegistered(address)

    @dbus.service.method('org.kde.StatusNotifierWatcher', in_signature='s', out_signature='')
    def RegisterStatusNotifierHost(self, host):
        self.StatusNotifierHostRegistered()

    @dbus.service.signal('org.kde.StatusNotifierWatcher', signature='s')
    def StatusNotifierItemRegistered(self, item):
        pass

    @dbus.service.signal('org.kde.StatusNotifierWatcher', signature='')
    def StatusNotifierHostRegistered(self):
        pass

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='ss', out_signature='v')
    def Get(self, interface, key):
        return self.GetAll(interface)[key]

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='s', out_signature='a{sv}')
    def GetAll(self, interface):
        return {'RegisteredStatusNotifierItems': dbus.Array(self.items, signature='s'), 'IsStatusNotifierHostRegistered': dbus.Boolean(True), 'ProtocolVersion': dbus.Int32(0)}

watcher = Watcher()
marker.with_suffix('.ready').write_text('ready')
GLib.MainLoop().run()
