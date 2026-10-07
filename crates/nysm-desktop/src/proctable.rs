//! Virtualized process table (GtkColumnView). Only visible rows have
//! widgets, so every process can be listed without a row cap. Row text
//! lives in a shared vector; refreshes update the cells that are on screen
//! in place, so scroll position and selection survive a refresh.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

/// (title, width in characters (0 = expanding), xalign).
/// NAME comes first so it stays visible when the window is narrow.
pub const COLUMNS: [(&str, i32, f32); 8] = [
    ("NAME", 0, 0.0),
    ("PID", 8, 1.0),
    ("USER", 10, 0.0),
    ("CPU %", 6, 1.0),
    ("MEM %", 6, 1.0),
    ("RSS", 9, 1.0),
    ("READ/s", 10, 1.0),
    ("WRITE/s", 10, 1.0),
];

pub type Cells = [String; COLUMNS.len()];

/// Cell widgets created so far: (list item, label, column).
type Bound = Vec<(
    glib::WeakRef<gtk::ListItem>,
    glib::WeakRef<gtk::Label>,
    usize,
)>;

#[derive(Clone)]
pub struct ProcTable {
    pub view: gtk::ColumnView,
    /// Placeholder items; only the count matters, text comes from `cells`.
    model: gtk::StringList,
    selection: gtk::MultiSelection,
    cells: Rc<RefCell<Vec<Cells>>>,
    bound: Rc<RefCell<Bound>>,
}

impl ProcTable {
    pub fn new() -> Self {
        let model = gtk::StringList::new(&[]);
        // Several rows can be selected (Ctrl/Shift-click) to watch them.
        let selection = gtk::MultiSelection::new(Some(model.clone()));
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_reorderable(false);
        view.set_show_row_separators(true);
        view.add_css_class("proc-table");
        let cells: Rc<RefCell<Vec<Cells>>> = Rc::default();
        let bound: Rc<RefCell<Bound>> = Rc::default();
        for (col, (title, width, xalign)) in COLUMNS.into_iter().enumerate() {
            let factory = gtk::SignalListItemFactory::new();
            let b = bound.clone();
            factory.connect_setup(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let l = gtk::Label::new(None);
                l.set_xalign(xalign);
                l.add_css_class("numeric");
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                l.set_width_chars(if width > 0 { width } else { 16 });
                item.set_child(Some(&l));
                b.borrow_mut().push((item.downgrade(), l.downgrade(), col));
            });
            let c = cells.clone();
            factory.connect_bind(move |_, item| {
                let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                if let Some(l) = item.child().and_downcast::<gtk::Label>() {
                    let cells = c.borrow();
                    l.set_text(
                        cells
                            .get(item.position() as usize)
                            .map_or("", |r| r[col].as_str()),
                    );
                }
            });
            let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
            column.set_resizable(true);
            column.set_expand(width == 0);
            view.append_column(&column);
        }
        ProcTable {
            view,
            model,
            selection,
            cells,
            bound,
        }
    }

    /// Replace all rows. Rows already on screen are updated in place; only
    /// the change in row count is signalled to the view.
    pub fn set_rows(&self, rows: Vec<Cells>) {
        let old = self.model.n_items();
        let new = rows.len() as u32;
        *self.cells.borrow_mut() = rows;
        if new < old {
            self.model.splice(new, old - new, &[]);
        } else if new > old {
            let add = vec![""; (new - old) as usize];
            self.model.splice(old, 0, &add);
        }
        let cells = self.cells.borrow();
        self.bound.borrow_mut().retain(|(item, label, col)| {
            let (Some(item), Some(label)) = (item.upgrade(), label.upgrade()) else {
                return false;
            };
            // Unbound (recycled) items report an invalid position: skipped.
            if let Some(r) = cells.get(item.position() as usize)
                && label.text() != r[*col].as_str()
            {
                label.set_text(&r[*col]);
            }
            true
        });
    }

    /// Indices of the selected rows.
    pub fn selected_rows(&self) -> Vec<usize> {
        let set = self.selection.selection();
        (0..set.size())
            .map(|i| set.nth(i as u32) as usize)
            .collect()
    }

    /// Select exactly these rows.
    pub fn select_rows(&self, rows: &[usize]) {
        self.selection.unselect_all();
        for &r in rows {
            self.selection.select_item(r as u32, false);
        }
    }

    /// Clear the selection (rows are re-sorted on every refresh, so a
    /// selection is only meaningful until it has been acted on).
    pub fn unselect_all(&self) {
        self.selection.unselect_all();
    }

    pub fn connect_selection_changed(&self, f: impl Fn(usize) + 'static) {
        self.selection
            .connect_selection_changed(move |sel, _, _| f(sel.selection().size() as usize));
    }

    /// Called with the row index on double-click or Enter.
    pub fn connect_activate(&self, f: impl Fn(usize) + 'static) {
        self.view.connect_activate(move |_, pos| f(pos as usize));
    }
}
