use std::cell::{Cell, RefCell};
use std::io;
use std::process::ExitCode;
use std::ptr::{self, NonNull};

use block2::RcBlock;
use objc2::rc::{Retained, Weak, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSMutableAttributedString, NSNotification, NSNumber, NSObject,
    NSObjectProtocol, NSPoint, NSRange, NSRect, NSRectEdge, NSSize, NSString, ns_string,
};

use crate::store::{Store, Task};

const WIDTH: f64 = 344.0;
const LIST_TOP: f64 = 8.0;
const MAX_LIST_HEIGHT: f64 = 260.0;
const ROW_WIDTH: f64 = WIDTH - 34.0;
const NAME_X: f64 = 20.0;
const NAME_WIDTH: f64 = ROW_WIDTH - NAME_X - 34.0;

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "DoohickeyContentView"]
    struct ContentView;
    impl ContentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }
    }
);

impl ContentView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        // Leave the background clear so NSPopover supplies the translucent material.
        unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Add,
    Toggle,
    Delete,
    Up,
    Down,
}

impl Shortcut {
    fn from_event(event: &NSEvent) -> Option<Self> {
        let chars = event
            .charactersIgnoringModifiers()
            .filter(|s| s.length() == 1)?;
        Self::from_key(u32::from(chars.characterAtIndex(0)), event.modifierFlags())
    }

    #[allow(non_upper_case_globals)] // AppKit names its character constants in mixed case.
    fn from_key(character: u32, flags: NSEventModifierFlags) -> Option<Self> {
        if flags.intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Control
                | NSEventModifierFlags::Option,
        ) {
            return None;
        }
        match (character, flags.contains(NSEventModifierFlags::Shift)) {
            (NSCarriageReturnCharacter | NSEnterCharacter, true) => Some(Self::Add),
            (NSCarriageReturnCharacter | NSEnterCharacter, false) => Some(Self::Toggle),
            (NSDeleteFunctionKey, false) => Some(Self::Delete),
            (NSUpArrowFunctionKey, false) => Some(Self::Up),
            (NSDownArrowFunctionKey, false) => Some(Self::Down),
            _ => None,
        }
    }
}

struct Panel {
    store: RefCell<Store>,
    popover: Retained<NSPopover>,
    root: Retained<ContentView>,
    editor: Retained<NSTextField>,
    count: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    list: Retained<ContentView>,
    empty: Retained<NSTextField>,
    footer: Retained<ContentView>,
    rows: RefCell<Vec<TaskRow>>,
    editing_name: RefCell<Option<Retained<NSButton>>>,
    current_row: Cell<Option<usize>>,
    shortcut_monitor: RefCell<Option<Retained<AnyObject>>>,
}

struct TaskRow {
    view: Retained<ContentView>,
    checkbox: Retained<NSButton>,
    name: Retained<NSButton>,
    trash: Retained<NSButton>,
}

impl TaskRow {
    fn show_task(&self, task: &Task) {
        self.checkbox.setState(isize::from(task.done));
        self.name.setAttributedTitle(&task_title(task));
        self.checkbox.setEnabled(true);
        let text = NSString::from_str(&task.text);
        self.checkbox.setAccessibilityLabel(Some(&text));
        self.name.setToolTip(Some(&text));
        self.name
            .setAccessibilityLabel(Some(&NSString::from_str(&format!("Edit {}", task.text))));
        self.trash
            .setAccessibilityLabel(Some(&NSString::from_str(&format!("Delete {}", task.text))));
    }

    fn place(&self, index: usize, y: f64) -> f64 {
        let text_height = self
            .name
            .attributedTitle()
            .boundingRectWithSize_options_context(
                NSSize::new(NAME_WIDTH - 4.0, f64::MAX),
                NSStringDrawingOptions::UsesLineFragmentOrigin,
                None,
            )
            .size
            .height
            .ceil();
        let height = (text_height + 14.0).max(32.0);
        self.view.setFrame(rect(18.0, y, ROW_WIDTH, height));
        self.checkbox
            .setFrame(rect(0.0, (height - 20.0) / 2.0, 18.0, 20.0));
        self.name
            .setFrame(rect(NAME_X, 2.0, NAME_WIDTH, height - 4.0));
        self.trash
            .setFrame(rect(ROW_WIDTH - 24.0, (height - 24.0) / 2.0, 24.0, 24.0));
        self.checkbox.setTag(index as isize);
        self.name.setTag(index as isize);
        self.trash.setTag(index as isize);
        height
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DoohickeyController"]
    #[ivars = Panel]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}
    unsafe impl NSTextFieldDelegate for Controller {}
    unsafe impl NSControlTextEditingDelegate for Controller {
        #[unsafe(method(controlTextDidEndEditing:))]
        fn editing_ended(&self, _note: &NSNotification) {
            self.end_edit(true);
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn editor_command(&self, _control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            if command == sel!(cancelOperation:) {
                self.end_edit(false);
                true
            } else {
                false
            }
        }
    }

    unsafe impl NSPopoverDelegate for Controller {
        #[unsafe(method(popoverDidShow:))]
        fn popover_did_show(&self, _note: &NSNotification) {
            self.install_shortcuts();
        }

        #[unsafe(method(popoverWillClose:))]
        fn popover_will_close(&self, _note: &NSNotification) {
            if let Some(monitor) = self.ivars().shortcut_monitor.take() {
                unsafe { NSEvent::removeMonitor(&monitor) };
            }
            if self.end_edit(true) {
                self.ivars().current_row.set(None);
            }
        }
    }

    unsafe impl NSApplicationDelegate for Controller {
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            if self.end_edit(true) {
                NSApplicationTerminateReply::TerminateNow
            } else {
                NSApplicationTerminateReply::TerminateCancel
            }
        }
    }

    impl Controller {
        #[unsafe(method(togglePopover:))]
        fn toggle_popover(&self, button: &NSStatusBarButton) {
            let panel = self.ivars();
            if panel.popover.isShown() {
                panel.popover.close();
            } else {
                let app = NSApplication::sharedApplication(self.mtm());
                app.activate();
                panel.popover.showRelativeToRect_ofView_preferredEdge(
                    button.bounds(), button, NSRectEdge::MinY,
                );
                if let Some(window) = panel.root.window() {
                    window.makeKeyWindow();
                    if panel.editing_name.borrow().is_some() {
                        window.makeFirstResponder(Some(&panel.editor));
                    }
                }
            }
        }

        #[unsafe(method(addTask:))]
        fn add_task(&self, _sender: &NSControl) {
            self.add_empty_row();
        }

        #[unsafe(method(editTask:))]
        fn edit_task(&self, sender: &NSButton) {
            if self.end_edit(true) {
                self.begin_edit(sender, None);
            }
        }

        #[unsafe(method(checkTask:))]
        fn check_task(&self, sender: &NSButton) {
            let index = sender.tag() as usize;
            let panel = self.ivars();
            let done = sender.state() == 1;
            let result = if self.end_edit(true) {
                panel.store.borrow_mut().set_done(index, done)
            } else {
                Ok(())
            };
            if let Some(task) = panel.store.borrow().tasks().get(index) {
                panel.current_row.set(Some(index));
                panel.rows.borrow()[index].show_task(task);
            }
            self.update_count();
            if let Err(error) = result {
                show_error(self.mtm(), "Couldn’t update this task", &error);
            }
        }

        #[unsafe(method(deleteTask:))]
        fn delete_task(&self, sender: &NSButton) {
            let index = sender.tag() as usize;
            if self.ivars().rows.borrow().get(index)
                .is_some_and(|row| std::ptr::eq(&*row.trash, sender))
            {
                self.delete_row(index);
            }
        }

        #[unsafe(method(showOptions:))]
        fn show_options(&self, sender: &NSButton) {
            let menu = quit_menu(self.mtm());
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::ZERO, Some(sender));
        }
    }
);

impl Controller {
    fn new(mtm: MainThreadMarker, store: Store) -> Retained<Self> {
        let root = ContentView::new(mtm, rect(0.0, 0.0, WIDTH, 150.0));
        let editor = NSTextField::textFieldWithString(ns_string!(""), mtm);
        editor.setPlaceholderString(Some(ns_string!("New task")));
        editor.setAccessibilityLabel(Some(ns_string!("Task name")));
        editor.setBezeled(false);
        editor.setBordered(false);
        editor.setDrawsBackground(false);
        editor.setFocusRingType(NSFocusRingType::None);
        editor.setUsesSingleLineMode(true);
        editor.setFont(Some(&NSFont::systemFontOfSize(13.0)));

        let scroll = NSScrollView::new(mtm);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let list = ContentView::new(mtm, NSRect::ZERO);
        scroll.setDocumentView(Some(&list));
        root.addSubview(&scroll);

        let empty = NSTextField::labelWithString(ns_string!("No tasks yet"), mtm);
        empty.setTextColor(Some(&NSColor::secondaryLabelColor()));
        empty.setAlignment(NSTextAlignment::Center);
        empty.setFrame(rect(16.0, 12.0, WIDTH - 32.0, 20.0));
        list.addSubview(&empty);

        let footer = ContentView::new(mtm, NSRect::ZERO);
        let count = NSTextField::labelWithString(ns_string!(""), mtm);
        count.setAlignment(NSTextAlignment::Right);
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        count.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            12.0, 0.0,
        )));
        count.setFrame(rect(WIDTH - 146.0, 9.0, 102.0, 17.0));
        footer.addSubview(&count);
        root.addSubview(&footer);

        let view_controller = NSViewController::new(mtm);
        view_controller.setView(&root);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        popover.setAnimates(false);
        popover.setContentViewController(Some(&view_controller));
        let this = Self::alloc(mtm).set_ivars(Panel {
            store: RefCell::new(store),
            popover,
            root,
            editor,
            count,
            scroll,
            list,
            empty,
            footer,
            rows: RefCell::default(),
            editing_name: RefCell::default(),
            current_row: Cell::new(None),
            shortcut_monitor: RefCell::default(),
        });
        // NSObject's designated initializer preserves the initialized Rust ivars.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let panel = this.ivars();
        // run() owns the delegates throughout the event loop.
        unsafe {
            panel
                .editor
                .setDelegate(Some(ProtocolObject::from_ref(&*this)));
        }
        panel
            .popover
            .setDelegate(Some(ProtocolObject::from_ref(&*this)));
        let add = this.icon_button("plus", "Add task", sel!(addTask:));
        add.setFrame(rect(12.0, 3.0, 28.0, 28.0));
        panel.footer.addSubview(&add);
        let options = this.icon_button("ellipsis.circle", "Options", sel!(showOptions:));
        options.setFrame(rect(WIDTH - 40.0, 4.0, 24.0, 26.0));
        panel.footer.addSubview(&options);
        for task in panel.store.borrow().tasks() {
            let row = this.new_row();
            row.show_task(task);
            panel.rows.borrow_mut().push(row);
        }
        this.layout_list();
        this
    }

    fn icon_button(&self, symbol: &str, label: &str, action: Sel) -> Retained<NSButton> {
        let label = NSString::from_str(label);
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(symbol),
            Some(&label),
        )
        .expect("the app uses built-in macOS symbols");
        // All button selectors take the sending control as their only argument.
        let button = unsafe {
            NSButton::buttonWithImage_target_action(&image, Some(self), Some(action), self.mtm())
        };
        button.setBordered(false);
        button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        button.setAccessibilityLabel(Some(&label));
        button.setToolTip(Some(&label));
        button
    }

    fn update_count(&self) {
        let panel = self.ivars();
        let store = panel.store.borrow();
        let open = store.tasks().iter().filter(|task| !task.done).count();
        let count = if store.tasks().is_empty() {
            String::new()
        } else if open == 0 {
            "All done".into()
        } else {
            format!("{open} open")
        };
        panel.count.setStringValue(&NSString::from_str(&count));
    }

    fn install_shortcuts(&self) {
        let controller = Weak::new(self);
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            // AppKit dispatches local monitors on the main thread with a live event.
            if controller
                .load()
                .is_some_and(|app| app.handle_shortcut(unsafe { event.as_ref() }))
            {
                ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &handler)
        };
        self.ivars().shortcut_monitor.replace(monitor);
    }

    fn handle_shortcut(&self, event: &NSEvent) -> bool {
        let panel = self.ivars();
        let Some(window) = panel.root.window() else {
            return false;
        };
        if !window.isKeyWindow() || event.windowNumber() != window.windowNumber() {
            return false;
        }
        let Some(shortcut) = Shortcut::from_event(event) else {
            return false;
        };
        let editor = panel
            .editor
            .currentEditor()
            .and_then(|editor| editor.downcast::<NSTextView>().ok());
        if editor
            .as_ref()
            .is_some_and(|view| NSTextInputClient::hasMarkedText(&**view))
        {
            return false;
        }
        if event.isARepeat()
            && matches!(
                shortcut,
                Shortcut::Add | Shortcut::Toggle | Shortcut::Delete
            )
        {
            return true;
        }
        if shortcut == Shortcut::Add {
            self.add_empty_row();
            return true;
        }
        let caret = editor
            .as_ref()
            .map_or(0, |view| NSText::selectedRange(view).location);
        let rows = panel.rows.borrow();
        let Some(last) = rows.len().checked_sub(1) else {
            return true;
        };
        let focused = window
            .firstResponder()
            .and_then(|r| r.downcast::<NSView>().ok());
        let current = panel
            .editing_name
            .borrow()
            .as_ref()
            .map(|name| name.tag() as usize)
            .or_else(|| {
                focused
                    .and_then(|focus| rows.iter().position(|row| focus.isDescendantOf(&row.view)))
            })
            .or(panel.current_row.get());
        if shortcut == Shortcut::Delete {
            let Some(index) = current else { return true };
            let next = rows
                .get(index + 1)
                .or_else(|| rows[..index].last())
                .map(|row| row.name.clone());
            drop(rows);
            if self.delete_row(index)
                && let Some(name) = next
            {
                self.begin_edit(&name, Some(caret));
            }
            return true;
        }
        let index = match (shortcut, current) {
            (Shortcut::Up, Some(i)) => i.saturating_sub(1),
            (Shortcut::Up, None) => last,
            (Shortcut::Down, Some(i)) => (i + 1).min(last),
            _ => current.unwrap_or(0),
        };
        if shortcut == Shortcut::Toggle {
            let checkbox = rows[index].checkbox.clone();
            drop(rows);
            // An unnamed draft disappears when editing ends and its checkbox stays disabled.
            if self.end_edit(true) && checkbox.isEnabled() {
                unsafe { checkbox.performClick(None) };
                window.makeFirstResponder(Some(&checkbox));
            }
        } else if current != Some(index) || editor.is_none() {
            let name = rows[index].name.clone();
            drop(rows);
            if self.end_edit(true) {
                self.begin_edit(&name, Some(caret));
            }
        }
        true
    }

    fn add_empty_row(&self) {
        if !self.end_edit(true) {
            return;
        }
        let panel = self.ivars();
        let row = self.new_row();
        let name = row.name.clone();
        row.checkbox.setEnabled(false);
        panel.rows.borrow_mut().push(row);
        self.layout_list();
        self.begin_edit(&name, Some(0));
    }

    fn remove_row(&self, index: usize) {
        let panel = self.ivars();
        let row = panel.rows.borrow_mut().remove(index);
        row.view.removeFromSuperview();
        let last = panel.rows.borrow().len().checked_sub(1);
        panel
            .current_row
            .set(panel.current_row.get().zip(last).map(|(current, last)| {
                if current > index {
                    current - 1
                } else {
                    current.min(last)
                }
            }));
    }

    fn delete_row(&self, index: usize) -> bool {
        let panel = self.ivars();
        // A draft exists only in the list until its first edit is saved.
        if index < panel.store.borrow().tasks().len() {
            let result = panel.store.borrow_mut().remove(index);
            if let Err(error) = result {
                show_error(self.mtm(), "Couldn’t delete this task", &error);
                return false;
            }
        }
        if panel
            .editing_name
            .borrow()
            .as_ref()
            .is_some_and(|name| name.tag() as usize == index)
        {
            panel.editing_name.take();
            self.detach_editor();
        }
        self.remove_row(index);
        self.layout_list();
        true
    }

    fn new_row(&self) -> TaskRow {
        let mtm = self.mtm();
        let view = ContentView::new(mtm, NSRect::ZERO);

        // checkTask: accepts the checkbox sending this action.
        let checkbox = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!(""),
                Some(self),
                Some(sel!(checkTask:)),
                mtm,
            )
        };
        view.addSubview(&checkbox);
        // The name is a separate target so activating it never changes completion.
        let name = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!(""),
                Some(self),
                Some(sel!(editTask:)),
                mtm,
            )
        };
        name.setBordered(false);
        name.setAlignment(NSTextAlignment::Left);
        name.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        if let Some(cell) = name.cell() {
            cell.setWraps(true);
        }
        view.addSubview(&name);
        let trash = self.icon_button("trash", "Delete task", sel!(deleteTask:));
        view.addSubview(&trash);
        self.ivars().list.addSubview(&view);
        TaskRow {
            view,
            checkbox,
            name,
            trash,
        }
    }

    fn begin_edit(&self, sender: &NSButton, caret: Option<usize>) {
        let panel = self.ivars();
        let (name, view) = {
            let rows = panel.rows.borrow();
            let Some(row) = rows.get(sender.tag() as usize) else {
                return;
            };
            (row.name.clone(), row.view.clone())
        };
        panel
            .editor
            .setStringValue(&name.attributedTitle().string());
        panel.editor.setFrame(rect(
            NAME_X,
            (view.frame().size.height - 18.0) / 2.0,
            NAME_WIDTH,
            18.0,
        ));
        name.setHidden(true);
        panel.current_row.set(Some(sender.tag() as usize));
        panel.editing_name.replace(Some(name));
        view.addSubview(&panel.editor);
        panel.editor.scrollRectToVisible(panel.editor.bounds());
        if let Some(window) = view.window() {
            window.makeFirstResponder(Some(&panel.editor));
            if let Some(editor) = panel.editor.currentEditor() {
                let text = editor.string();
                let selection = match caret {
                    None => NSRange::new(0, text.length()),
                    Some(offset) if offset < text.length() => NSRange::new(
                        text.rangeOfComposedCharacterSequenceAtIndex(offset)
                            .location,
                        0,
                    ),
                    Some(_) => NSRange::new(text.length(), 0),
                };
                editor.setSelectedRange(selection);
            }
        }
    }

    fn end_edit(&self, save: bool) -> bool {
        let panel = self.ivars();
        // Ending the field editor may immediately call the delegate again.
        let Some(name) = panel.editing_name.take() else {
            return true;
        };
        let index = name.tag() as usize;
        if save {
            let text = panel
                .editor
                .currentEditor()
                .map_or_else(|| panel.editor.stringValue(), |editor| editor.string());
            let result = {
                let mut store = panel.store.borrow_mut();
                if index == store.tasks().len() {
                    store.add(&text.to_string()).map(|_| ())
                } else {
                    store.rename(index, &text.to_string())
                }
            };
            if let Err(error) = result {
                show_error(self.mtm(), "Couldn’t save this task", &error);
                panel.editor.setStringValue(&text);
                panel.editing_name.replace(Some(name));
                return false;
            }
        }
        self.detach_editor();
        if let Some(task) = panel.store.borrow().tasks().get(index) {
            panel.rows.borrow()[index].show_task(task);
            name.setHidden(false);
        } else {
            // The new row was cancelled or left empty.
            self.remove_row(index);
        }
        self.layout_list();
        true
    }

    fn detach_editor(&self) {
        let editor = &self.ivars().editor;
        editor.abortEditing();
        editor.removeFromSuperview();
    }

    fn layout_list(&self) {
        let panel = self.ivars();
        let mut height = 0.0;
        {
            let rows = panel.rows.borrow();
            for (index, row) in rows.iter().enumerate() {
                height += row.place(index, height);
            }
        }
        panel.empty.setHidden(height > 0.0);
        if height == 0.0 {
            height = 44.0;
        }
        let visible = height.min(MAX_LIST_HEIGHT);
        panel.scroll.setFrame(rect(0.0, LIST_TOP, WIDTH, visible));
        panel
            .list
            .setFrame(rect(0.0, 0.0, panel.scroll.contentSize().width, height));
        panel
            .footer
            .setFrame(rect(0.0, LIST_TOP + visible, WIDTH, 36.0));
        let size = NSSize::new(WIDTH, LIST_TOP + visible + 36.0);
        panel.root.setFrameSize(size);
        panel.popover.setContentSize(size);
        self.update_count();
    }
}

fn task_title(task: &Task) -> Retained<NSMutableAttributedString> {
    let text = NSString::from_str(&task.text);
    let title =
        NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &text);
    let range = NSRange::new(0, title.length());
    let ink = if task.done {
        NSColor::secondaryLabelColor()
    } else {
        NSColor::labelColor()
    };
    // Each attribute has its AppKit value type and the range covers the UTF-16 string.
    unsafe {
        title.addAttribute_value_range(NSFontAttributeName, &NSFont::systemFontOfSize(13.0), range);
        title.addAttribute_value_range(NSForegroundColorAttributeName, &ink, range);
        if task.done {
            title.addAttribute_value_range(
                NSStrikethroughStyleAttributeName,
                &NSNumber::numberWithInt(1),
                range,
            );
        }
    }
    title
}

fn quit_menu(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    // Route the standard terminate: action through the responder chain.
    unsafe {
        menu.addItemWithTitle_action_keyEquivalent(
            ns_string!("Quit doohickey"),
            Some(sel!(terminate:)),
            ns_string!("q"),
        )
    };
    menu
}

fn install_menu(app: &NSApplication, mtm: MainThreadMarker) {
    let menu = NSMenu::new(mtm);
    let app_item = NSMenuItem::new(mtm);
    app_item.setSubmenu(Some(&quit_menu(mtm)));
    menu.addItem(&app_item);
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Edit"));
    for (label, action, key) in [
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
        ("Undo", sel!(undo:), "z"),
    ] {
        // These are standard one-argument actions on the active text responder.
        unsafe {
            edit.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(label),
                Some(action),
                &NSString::from_str(key),
            )
        };
    }
    let edit_item = NSMenuItem::new(mtm);
    edit_item.setSubmenu(Some(&edit));
    menu.addItem(&edit_item);
    app.setMainMenu(Some(&menu));
}

fn show_error(mtm: MainThreadMarker, title: &str, error: &io::Error) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(&error.to_string()));
    alert.runModal();
}

pub fn run() -> ExitCode {
    let mtm = MainThreadMarker::new().expect("AppKit must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let store = std::env::home_dir()
        .ok_or_else(|| io::Error::other("Home folder unavailable"))
        .and_then(|home| Store::open(&home.join("Library/Application Support/doohickey")));
    let store = match store {
        Ok(store) => store,
        Err(error) => {
            show_error(mtm, "Couldn’t open doohickey", &error);
            return ExitCode::FAILURE;
        }
    };
    let (_controller, _status) = autoreleasepool(|_| {
        install_menu(&app, mtm);
        let controller = Controller::new(mtm, store);
        app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
        let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSSquareStatusItemLength);
        let button = status.button(mtm).expect("status item has a button");
        let name = ns_string!("doohickey");
        let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("checkmark.square"),
            Some(name),
        )
        .expect("checkmark.square is a built-in macOS symbol");
        image.setTemplate(true);
        button.setImage(Some(&image));
        button.setToolTip(Some(name));
        button.setAccessibilityLabel(Some(name));
        // The status button is the sender expected by togglePopover:.
        unsafe {
            button.setTarget(Some(&controller));
            button.setAction(Some(sel!(togglePopover:)));
        }
        (controller, status)
    });
    app.run();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_delete_removes_rows_and_backspace_goes_to_the_editor() {
        let plain = NSEventModifierFlags::empty();
        for flags in [plain, NSEventModifierFlags::Function] {
            assert!(Shortcut::from_key(NSDeleteFunctionKey, flags) == Some(Shortcut::Delete));
            assert!(Shortcut::from_key(NSDeleteCharacter, flags).is_none());
            assert!(Shortcut::from_key(NSBackspaceCharacter, flags).is_none());
        }
        for modifier in [
            NSEventModifierFlags::Command,
            NSEventModifierFlags::Option,
            NSEventModifierFlags::Control,
            NSEventModifierFlags::Shift,
        ] {
            assert!(Shortcut::from_key(NSDeleteFunctionKey, modifier).is_none());
        }
    }
}
