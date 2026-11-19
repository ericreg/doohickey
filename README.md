# doohickey

A small Rust checklist for the macOS menu bar. Open the checkmark, then click **+** to add a task or click a name to edit it in place. Click a checkbox to toggle completion or the trash icon to delete the task.

| Shortcut | Action |
| --- | --- |
| **Shift + Enter** | Save the current edit and start a blank task at the bottom. |
| **↑ / ↓** | Save the current edit and move the text cursor to the previous or next task. |
| **Enter** | Save the current edit and check or uncheck the current task. |
| **Forward Delete (⌦)** or **Fn + ⌫** | Delete the current row. The cursor moves to the next task, or to the previous task at the end of the list. |
| **Escape** | Cancel the current edit. |

Edits also save when you leave the row or close the popover. Empty drafts are discarded.

## Run

On macOS 14 or later, with Rust 1.89 or later and the Xcode Command Line Tools installed, run `cargo run --release`.

For a double-clickable app, run `just app` and open **target/doohickey.app**. You can move the bundle to Applications. Choose **Quit doohickey** from the popover’s **…** menu to exit.

Tasks are saved locally in `~/Library/Application Support/doohickey/tasks.json`.
