//! An icon in the notification area while Pipedeck keeps running without
//! its window: a click opens the window, its menu opens it or quits.
//!
//! GTK 4 has no such icon, so it is the desktop's own protocol, spoken over
//! D-Bus on the application's connection: a StatusNotifierItem, which KDE
//! and others with an extension show, and its menu as a dbusmenu. Owning
//! the item's name is what makes it appear; releasing it, what takes it
//! away.

use std::cell::RefCell;
use std::collections::HashMap;

use adw::gtk::{gio, glib};
use adw::prelude::*;
use libadwaita as adw;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_XML: &str = r#"<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="IconThemePath" type="s" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <property name="WindowId" type="i" access="read"/>
    <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
    <method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
    <method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
    <method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
    <method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method>
  </interface>
</node>"#;
const MENU_XML: &str = r#"<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/>
      <arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg type="ai" direction="in"/><arg type="as" direction="in"/>
      <arg type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/>
    </method>
    <method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
    <method name="AboutToShowGroup">
      <arg type="ai" direction="in"/><arg type="ai" direction="out"/><arg type="ai" direction="out"/>
    </method>
    <signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal>
  </interface>
</node>"#;

/// The menu's entries, by their id: the root is 0.
const OPEN: i32 = 1;
const QUIT: i32 = 2;

/// What is on the bus while the icon shows.
struct Shown {
    connection: gio::DBusConnection,
    registrations: Vec<gio::RegistrationId>,
    name: gio::OwnerId,
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// One entry of the menu, with what a dbusmenu says of it.
fn entry(id: i32, label: &str) -> glib::Variant {
    let mut props: HashMap<String, glib::Variant> = HashMap::new();
    props.insert("label".into(), label.to_variant());
    props.insert("enabled".into(), true.to_variant());
    props.insert("visible".into(), true.to_variant());
    (id, props, Vec::<glib::Variant>::new()).to_variant()
}

fn properties(id: i32) -> HashMap<String, glib::Variant> {
    let mut props: HashMap<String, glib::Variant> = HashMap::new();
    match id {
        OPEN => {
            props.insert("label".into(), "Open Pipedeck".to_variant());
        }
        QUIT => {
            props.insert("label".into(), "Quit".to_variant());
        }
        _ => {
            props.insert("children-display".into(), "submenu".to_variant());
        }
    }
    props
}

/// The whole menu: a root holding the two entries.
fn layout() -> glib::Variant {
    let children = vec![
        glib::Variant::from_variant(&entry(OPEN, "Open Pipedeck")),
        glib::Variant::from_variant(&entry(QUIT, "Quit")),
    ];
    let mut root: HashMap<String, glib::Variant> = HashMap::new();
    root.insert("children-display".into(), "submenu".to_variant());
    (0i32, root, children).to_variant()
}

fn icon_theme_path() -> String {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
        .map(|data| data.join("icons").display().to_string())
        .unwrap_or_default()
}

/// Show the icon, or take it away.
pub fn show(app: &adw::Application, on: bool) {
    let shown = SHOWN.with(|shown| shown.borrow().is_some());
    if on == shown {
        return;
    }
    if !on {
        hide();
        return;
    }
    if let Some(connection) = app.dbus_connection() {
        show_on(app, &connection);
    }
}

/// Put the icon and its menu on a bus.
fn show_on(app: &adw::Application, connection: &gio::DBusConnection) {
    let (Some(item), Some(menu)) = (
        gio::DBusNodeInfo::for_xml(ITEM_XML)
            .ok()
            .and_then(|node| node.lookup_interface("org.kde.StatusNotifierItem")),
        gio::DBusNodeInfo::for_xml(MENU_XML)
            .ok()
            .and_then(|node| node.lookup_interface("com.canonical.dbusmenu")),
    ) else {
        return;
    };
    let app_id = app.application_id().map(String::from).unwrap_or_default();

    let item_registration = connection
        .register_object(ITEM_PATH, &item)
        .method_call({
            let app = app.clone();
            move |_, _, _, _, method, _, invocation| {
                if method == "Activate" || method == "SecondaryActivate" {
                    app.activate();
                }
                invocation.return_value(None);
            }
        })
        .property(move |_, _, _, _, property| item_property(&app_id, property))
        .build();
    let menu_registration = connection
        .register_object(MENU_PATH, &menu)
        .method_call({
            let app = app.clone();
            move |_, _, _, _, method, params, invocation| {
                invocation.return_value(menu_reply(&app, method, &params).as_ref());
            }
        })
        .property(|_, _, _, _, property| menu_property(property))
        .build();
    let (Ok(item_registration), Ok(menu_registration)) = (item_registration, menu_registration)
    else {
        log::warn!("cannot put Pipedeck in the notification area");
        return;
    };

    // The item's name: owning it is what the area looks for, and the
    // watcher is told of it once it is ours.
    let service = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let name = gio::bus_own_name_on_connection(
        connection,
        &service,
        gio::BusNameOwnerFlags::NONE,
        {
            let service = service.clone();
            move |connection, _| tell_watcher(&connection, &service)
        },
        |_, _| {},
    );
    SHOWN.with(|shown| {
        *shown.borrow_mut() = Some(Shown {
            connection: connection.clone(),
            registrations: vec![item_registration, menu_registration],
            name,
        });
    });
}

/// Take the icon away: release its name and its objects.
fn hide() {
    SHOWN.with(|shown| {
        let Some(shown) = shown.borrow_mut().take() else {
            return;
        };
        gio::bus_unown_name(shown.name);
        for id in shown.registrations {
            let _ = shown.connection.unregister_object(id);
        }
    });
}

/// What the icon says of itself.
fn item_property(app_id: &str, property: &str) -> glib::Variant {
    match property {
        "Category" => "ApplicationStatus".to_variant(),
        "Id" => "pipedeck".to_variant(),
        "Title" => "Pipedeck".to_variant(),
        "Status" => "Active".to_variant(),
        "IconName" => app_id.to_variant(),
        "IconThemePath" => icon_theme_path().to_variant(),
        "ItemIsMenu" => false.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH)
            .expect("a valid path")
            .to_variant(),
        "WindowId" => 0i32.to_variant(),
        _ => (
            app_id.to_owned(),
            Vec::<(i32, i32, Vec<u8>)>::new(),
            "Pipedeck".to_owned(),
            "The mixer keeps running".to_owned(),
        )
            .to_variant(),
    }
}

/// What the menu says of itself.
fn menu_property(property: &str) -> glib::Variant {
    match property {
        "Version" => 3u32.to_variant(),
        "TextDirection" => "ltr".to_variant(),
        "Status" => "normal".to_variant(),
        _ => Vec::<String>::new().to_variant(),
    }
}

/// The menu's answer to one of its methods.
fn menu_reply(
    app: &adw::Application,
    method: &str,
    params: &glib::Variant,
) -> Option<glib::Variant> {
    match method {
        "GetLayout" => Some(glib::Variant::tuple_from_iter([
            1u32.to_variant(),
            layout(),
        ])),
        "GetGroupProperties" => {
            let ids: Vec<i32> = params.child_value(0).get().unwrap_or_default();
            let group: Vec<(i32, HashMap<String, glib::Variant>)> =
                ids.into_iter().map(|id| (id, properties(id))).collect();
            Some((group,).to_variant())
        }
        "GetProperty" => {
            let id: i32 = params.child_value(0).get().unwrap_or_default();
            let name: String = params.child_value(1).get().unwrap_or_default();
            let value = properties(id)
                .remove(&name)
                .unwrap_or_else(|| "".to_variant());
            Some((glib::Variant::from_variant(&value),).to_variant())
        }
        "Event" => {
            let id: i32 = params.child_value(0).get().unwrap_or_default();
            let event: String = params.child_value(1).get().unwrap_or_default();
            happened(app, id, &event);
            None
        }
        "EventGroup" => {
            let events: Vec<(i32, String, glib::Variant, u32)> =
                params.child_value(0).get().unwrap_or_default();
            for (id, event, _, _) in events {
                happened(app, id, &event);
            }
            Some((Vec::<i32>::new(),).to_variant())
        }
        "AboutToShow" => Some((false,).to_variant()),
        "AboutToShowGroup" => Some((Vec::<i32>::new(), Vec::<i32>::new()).to_variant()),
        _ => None,
    }
}

/// Do what an entry of the menu is for, once clicked.
fn happened(app: &adw::Application, id: i32, event: &str) {
    if event != "clicked" {
        return;
    }
    match id {
        OPEN => app.activate(),
        QUIT => app.quit(),
        _ => {}
    }
}

/// Tell the notification area the icon is there.
fn tell_watcher(connection: &gio::DBusConnection, service: &str) {
    let registered = connection.call_sync(
        Some("org.kde.StatusNotifierWatcher"),
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
        "RegisterStatusNotifierItem",
        Some(&(service,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
    );
    if let Err(e) = registered {
        log::warn!("no notification area to show Pipedeck in: {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn the_menu_is_laid_out_as_dbusmenu_has_it() {
        let reply = glib::Variant::tuple_from_iter([1u32.to_variant(), layout()]);
        assert_eq!(reply.type_().as_str(), "(u(ia{sv}av))");
        assert_eq!(layout().child_value(2).n_children(), 2);
    }

    #[test]
    fn the_menu_answers_as_dbusmenu_expects() {
        let app = adw::Application::builder()
            .application_id("dev._2c2t.PipedeckTrayTest")
            .build();
        let reply = |method: &str, params: glib::Variant| {
            menu_reply(&app, method, &params).map(|v| v.type_().as_str().to_owned())
        };
        assert_eq!(
            reply(
                "GetLayout",
                (0i32, -1i32, Vec::<String>::new()).to_variant()
            )
            .as_deref(),
            Some("(u(ia{sv}av))")
        );
        assert_eq!(
            reply(
                "GetGroupProperties",
                (vec![OPEN, QUIT], Vec::<String>::new()).to_variant()
            )
            .as_deref(),
            Some("(a(ia{sv}))")
        );
        assert_eq!(
            reply("GetProperty", (OPEN, "label").to_variant()).as_deref(),
            Some("(v)")
        );
        assert_eq!(
            reply(
                "EventGroup",
                (Vec::<(i32, String, glib::Variant, u32)>::new(),).to_variant()
            )
            .as_deref(),
            Some("(ai)")
        );
        assert_eq!(
            reply("AboutToShow", (0i32,).to_variant()).as_deref(),
            Some("(b)")
        );
        assert_eq!(
            reply("AboutToShowGroup", (Vec::<i32>::new(),).to_variant()).as_deref(),
            Some("(aiai)")
        );
        assert_eq!(
            reply(
                "Event",
                (0i32, "hovered", 0i32.to_variant(), 0u32).to_variant()
            ),
            None
        );
        assert_eq!(
            item_property("dev._2c2t.Pipedeck", "IconName").str(),
            Some("dev._2c2t.Pipedeck")
        );
        assert_eq!(menu_property("Version").get::<u32>(), Some(3));
    }

    /// A bus of the test's own, so nothing reaches the desktop's: a
    /// dbus-daemon, stopped once the test is done with it.
    struct PrivateBus {
        daemon: std::process::Child,
        address: String,
    }

    impl PrivateBus {
        fn start() -> Option<Self> {
            use std::io::BufRead;
            let mut daemon = std::process::Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .ok()?;
            let mut address = String::new();
            std::io::BufReader::new(daemon.stdout.take()?)
                .read_line(&mut address)
                .ok()?;
            Some(PrivateBus {
                daemon,
                address: address.trim().to_owned(),
            })
        }

        fn connect(&self) -> gio::DBusConnection {
            gio::DBusConnection::for_address_sync(
                &self.address,
                gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                    | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
                None,
                gio::Cancellable::NONE,
            )
            .expect("the private bus answers")
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    const WATCHER_XML: &str = r#"<node>
      <interface name="org.kde.StatusNotifierWatcher">
        <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
      </interface>
    </node>"#;

    fn call(
        connection: &gio::DBusConnection,
        to: &str,
        path: &str,
        interface: &str,
        method: &str,
        args: glib::Variant,
    ) -> glib::Variant {
        connection
            .call_sync(
                Some(to),
                path,
                interface,
                method,
                Some(&args),
                None,
                gio::DBusCallFlags::NONE,
                5000,
                gio::Cancellable::NONE,
            )
            .unwrap_or_else(|e| panic!("{interface}.{method}: {e}"))
    }

    /// What the notification area makes of the icon: who told it, the
    /// icon's id and menu, and how many entries the menu has.
    type Seen = (String, String, String, usize);

    /// A notification area, on a thread of its own as it is a process of
    /// its own on a desktop: it waits to be told of an icon, then reads it.
    fn notification_area(address: String, ready: mpsc::Sender<()>, seen: mpsc::Sender<Seen>) {
        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                let connection = gio::DBusConnection::for_address_sync(
                    &address,
                    gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
                        | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
                    None,
                    gio::Cancellable::NONE,
                )
                .unwrap();
                let told = std::rc::Rc::new(RefCell::new(None::<String>));
                let watcher = gio::DBusNodeInfo::for_xml(WATCHER_XML).unwrap();
                let _registration = connection
                    .register_object(
                        "/StatusNotifierWatcher",
                        &watcher
                            .lookup_interface("org.kde.StatusNotifierWatcher")
                            .unwrap(),
                    )
                    .method_call({
                        let told = told.clone();
                        move |_, _, _, _, _, params, invocation| {
                            *told.borrow_mut() = params.child_value(0).get();
                            invocation.return_value(None);
                        }
                    })
                    .build()
                    .unwrap();
                call(
                    &connection,
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "RequestName",
                    ("org.kde.StatusNotifierWatcher", 0u32).to_variant(),
                );
                ready.send(()).unwrap();

                let end = Instant::now() + Duration::from_secs(10);
                while told.borrow().is_none() && Instant::now() < end {
                    context.iteration(false);
                    std::thread::sleep(Duration::from_millis(5));
                }
                let Some(service) = told.take() else {
                    return;
                };
                let property = |name: &str| {
                    call(
                        &connection,
                        &service,
                        ITEM_PATH,
                        "org.freedesktop.DBus.Properties",
                        "Get",
                        ("org.kde.StatusNotifierItem", name).to_variant(),
                    )
                    .child_value(0)
                    .as_variant()
                    .unwrap()
                };
                let id: String = property("Id").get().unwrap();
                let menu = property("Menu").str().unwrap().to_owned();
                let layout = call(
                    &connection,
                    &service,
                    &menu,
                    "com.canonical.dbusmenu",
                    "GetLayout",
                    (0i32, -1i32, Vec::<String>::new()).to_variant(),
                );
                let entries = layout.child_value(1).child_value(2).n_children();
                seen.send((service, id, menu, entries)).unwrap();
            })
            .unwrap();
    }

    #[test]
    fn the_icon_is_put_on_a_bus_and_taken_off_it() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("no dbus-daemon here: skipped");
            return;
        };
        let (ready, is_ready) = mpsc::channel();
        let (seen, has_seen) = mpsc::channel();
        let area = {
            let address = bus.address.clone();
            std::thread::spawn(move || notification_area(address, ready, seen))
        };
        is_ready.recv().unwrap();

        let context = glib::MainContext::new();
        context
            .with_thread_default(|| {
                let connection = bus.connect();
                let app = adw::Application::builder()
                    .application_id("dev._2c2t.PipedeckTrayTest")
                    .build();
                show_on(&app, &connection);

                // The name is taken, the area told and the icon read while
                // this thread answers.
                let end = Instant::now() + Duration::from_secs(10);
                let seen = loop {
                    context.iteration(false);
                    if let Ok(seen) = has_seen.try_recv() {
                        break seen;
                    }
                    assert!(Instant::now() < end, "the notification area saw nothing");
                    std::thread::sleep(Duration::from_millis(5));
                };
                let service = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
                assert_eq!(
                    seen,
                    (service.clone(), "pipedeck".into(), MENU_PATH.into(), 2)
                );

                hide();
                let owned = call(
                    &connection,
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    "org.freedesktop.DBus",
                    "NameHasOwner",
                    (service,).to_variant(),
                );
                assert_eq!(owned.child_value(0).get::<bool>(), Some(false));
                assert!(SHOWN.with(|shown| shown.borrow().is_none()));
            })
            .unwrap();
        area.join().unwrap();
    }

    #[test]
    fn the_interfaces_are_read() {
        assert!(gio::DBusNodeInfo::for_xml(ITEM_XML).is_ok());
        assert!(gio::DBusNodeInfo::for_xml(MENU_XML).is_ok());
    }
}
