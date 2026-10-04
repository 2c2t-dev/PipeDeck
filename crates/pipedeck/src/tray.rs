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
        SHOWN.with(|shown| {
            if let Some(shown) = shown.borrow_mut().take() {
                gio::bus_unown_name(shown.name);
                if let Some(connection) = app.dbus_connection() {
                    for id in shown.registrations {
                        let _ = connection.unregister_object(id);
                    }
                }
            }
        });
        return;
    }
    let Some(connection) = app.dbus_connection() else {
        return;
    };
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
        .property({
            let app_id = app_id.clone();
            move |_, _, _, _, property| match property {
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
                    app_id.clone(),
                    Vec::<(i32, i32, Vec<u8>)>::new(),
                    "Pipedeck".to_owned(),
                    "The mixer keeps running".to_owned(),
                )
                    .to_variant(),
            }
        })
        .build();
    let menu_registration = connection
        .register_object(MENU_PATH, &menu)
        .method_call({
            let app = app.clone();
            move |_, _, _, _, method, params, invocation| match method {
                "GetLayout" => {
                    let reply = glib::Variant::tuple_from_iter([1u32.to_variant(), layout()]);
                    invocation.return_value(Some(&reply));
                }
                "GetGroupProperties" => {
                    let ids: Vec<i32> = params.child_value(0).get().unwrap_or_default();
                    let group: Vec<(i32, HashMap<String, glib::Variant>)> =
                        ids.into_iter().map(|id| (id, properties(id))).collect();
                    invocation.return_value(Some(&(group,).to_variant()));
                }
                "GetProperty" => {
                    let id: i32 = params.child_value(0).get().unwrap_or_default();
                    let name: String = params.child_value(1).get().unwrap_or_default();
                    let value = properties(id)
                        .remove(&name)
                        .unwrap_or_else(|| "".to_variant());
                    invocation
                        .return_value(Some(&(glib::Variant::from_variant(&value),).to_variant()));
                }
                "Event" => {
                    let id: i32 = params.child_value(0).get().unwrap_or_default();
                    let event: String = params.child_value(1).get().unwrap_or_default();
                    if event == "clicked" {
                        match id {
                            OPEN => app.activate(),
                            QUIT => app.quit(),
                            _ => {}
                        }
                    }
                    invocation.return_value(None);
                }
                "EventGroup" => {
                    let events: Vec<(i32, String, glib::Variant, u32)> =
                        params.child_value(0).get().unwrap_or_default();
                    for (id, event, _, _) in events {
                        if event == "clicked" {
                            match id {
                                OPEN => app.activate(),
                                QUIT => app.quit(),
                                _ => {}
                            }
                        }
                    }
                    invocation.return_value(Some(&(Vec::<i32>::new(),).to_variant()));
                }
                "AboutToShow" => invocation.return_value(Some(&(false,).to_variant())),
                "AboutToShowGroup" => invocation
                    .return_value(Some(&(Vec::<i32>::new(), Vec::<i32>::new()).to_variant())),
                _ => invocation.return_value(None),
            }
        })
        .property(move |_, _, _, _, property| match property {
            "Version" => 3u32.to_variant(),
            "TextDirection" => "ltr".to_variant(),
            "Status" => "normal".to_variant(),
            _ => Vec::<String>::new().to_variant(),
        })
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
        &connection,
        &service,
        gio::BusNameOwnerFlags::NONE,
        {
            let service = service.clone();
            move |connection, _| {
                let registered = connection.call_sync(
                    Some("org.kde.StatusNotifierWatcher"),
                    "/StatusNotifierWatcher",
                    "org.kde.StatusNotifierWatcher",
                    "RegisterStatusNotifierItem",
                    Some(&(service.clone(),).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    2000,
                    gio::Cancellable::NONE,
                );
                if let Err(e) = registered {
                    log::warn!("no notification area to show Pipedeck in: {e}");
                }
            }
        },
        |_, _| {},
    );
    SHOWN.with(|shown| {
        *shown.borrow_mut() = Some(Shown {
            registrations: vec![item_registration, menu_registration],
            name,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_is_laid_out_as_dbusmenu_has_it() {
        let reply = glib::Variant::tuple_from_iter([1u32.to_variant(), layout()]);
        assert_eq!(reply.type_().as_str(), "(u(ia{sv}av))");
        assert_eq!(layout().child_value(2).n_children(), 2);
    }

    #[test]
    fn the_interfaces_are_read() {
        assert!(gio::DBusNodeInfo::for_xml(ITEM_XML).is_ok());
        assert!(gio::DBusNodeInfo::for_xml(MENU_XML).is_ok());
    }
}
