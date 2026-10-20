//! Linux only: lets child webviews (the Store tab) sit at their bounds.
//!
//! wry packs every webview of a GTK window into the window's `GtkBox`, where
//! they split the height between them and ignore position and size: the store
//! page took the bottom half of the launcher. Here the main webview and the
//! store webviews are moved into one `GtkFixed` instead; the main webview is
//! stretched over all of it and each store webview is given its own bounds
//! by hand. Store webviews are added after the main one, so they draw (and
//! take input) on top of it.
//!
//! GTK may only be touched on the main thread: everything here runs from
//! `with_webview` / `run_on_main_thread` callbacks.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::prelude::*;
use tauri::webview::Webview;
use tauri::{AppHandle, Manager};

use crate::commands::storeview::Bounds;

struct Layer {
    fixed: gtk::Fixed,
    main: gtk::Widget,
    /// Store webviews by label, with their bounds (logical px, window-relative).
    children: HashMap<String, (gtk::Widget, Bounds)>,
}

thread_local! {
    static LAYER: RefCell<Option<Layer>> = const { RefCell::new(None) };
}

/// Put the main webview into the `GtkFixed`. Call once, after the main
/// window got its webview.
pub fn install(app: &AppHandle) {
    let Some(main) = app.get_webview(crate::MAIN_WINDOW) else {
        return;
    };
    let done = main.with_webview(|pw| {
        let main: gtk::Widget = pw.inner().upcast();
        let Some(vbox) = main.parent().and_then(|p| p.downcast::<gtk::Box>().ok()) else {
            tracing::warn!("main webview is not in a GtkBox; store pages keep wry's layout");
            return;
        };
        let fixed = gtk::Fixed::new();
        vbox.remove(&main);
        fixed.put(&main, 0, 0);
        vbox.pack_start(&fixed, true, true, 0);
        // the class handler gives every child its requested size; then the
        // main webview is stretched and the store webviews moved into place
        fixed.connect_size_allocate(|_, alloc| relayout(alloc));
        fixed.show_all();
        LAYER.with(|l| {
            *l.borrow_mut() = Some(Layer {
                fixed,
                main,
                children: HashMap::new(),
            })
        });
        tracing::debug!("webviews moved into a GtkFixed");
    });
    if let Err(e) = done {
        tracing::warn!(error = %e, "could not rearrange the main window's webviews");
    }
}

/// Move a newly created store webview into the layer at `bounds`.
pub fn adopt(webview: &Webview, bounds: Bounds) {
    let label = webview.label().to_string();
    let _ = webview.with_webview(move |pw| {
        let widget: gtk::Widget = pw.inner().upcast();
        LAYER.with(|l| {
            let mut l = l.borrow_mut();
            let Some(layer) = l.as_mut() else { return };
            if let Some(parent) = widget
                .parent()
                .and_then(|p| p.downcast::<gtk::Container>().ok())
            {
                parent.remove(&widget);
            }
            layer.fixed.put(&widget, 0, 0);
            layer.children.insert(label, (widget, bounds));
            layer.fixed.queue_resize();
        });
    });
}

/// New bounds for a store webview.
pub fn place(app: &AppHandle, label: String, bounds: Bounds) {
    let _ = app.run_on_main_thread(move || {
        LAYER.with(|l| {
            let mut l = l.borrow_mut();
            let Some(layer) = l.as_mut() else { return };
            if let Some(entry) = layer.children.get_mut(&label) {
                entry.1 = bounds;
                layer.fixed.queue_resize();
            }
        });
    });
}

/// Forget a store webview that is being closed.
pub fn forget(app: &AppHandle, label: String) {
    let _ = app.run_on_main_thread(move || {
        LAYER.with(|l| {
            if let Some(layer) = l.borrow_mut().as_mut() {
                layer.children.remove(&label);
            }
        });
    });
}

fn relayout(alloc: &gtk::Allocation) {
    LAYER.with(|l| {
        let Ok(l) = l.try_borrow() else { return };
        let Some(layer) = l.as_ref() else { return };
        layer.main.size_allocate(alloc);
        for (widget, b) in layer.children.values() {
            if !widget.is_visible() {
                continue;
            }
            // a GtkFixed has no window of its own: children are placed in
            // the coordinates of the window below it
            let rect = gtk::Allocation::new(
                alloc.x() + b.x.round() as i32,
                alloc.y() + b.y.round() as i32,
                b.width.round().max(1.0) as i32,
                b.height.round().max(1.0) as i32,
            );
            widget.size_allocate(&rect);
        }
    });
}
