# Terminal User Interface (TUI)

exo features an interactive terminal user interface built on Bubble Tea, providing full parity with the storage repository.

## Starting the TUI

To start the TUI mode, run the `exo` command with no arguments:

```bash
exo
```

## Keybindings

The TUI uses intuitive, single-key commands:

| Key | Action |
| --- | --- |
| `n` | Create a new item (interactive wizard using templates) |
| `e` / `Enter` | Open the item in your editor (`$EDITOR`) |
| `d` / `Delete` | Delete the currently selected item |
| `a` / `Space` | Open custom actions context menu for the selected item |
| `Tab` | Cycle forwards through subview tabs |
| `Shift + Tab` | Cycle backwards through subview tabs |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `/` | Focus search bar to filter list |
| `Esc` | Clear search or close modals |
| `q` / `Ctrl + C` | Exit the TUI |
