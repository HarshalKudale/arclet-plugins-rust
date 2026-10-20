# VS Code Plugin for Arclet

Arclet companion plugin for **Visual Studio Code**. It talks to the **Arclet VS Code Companion** extension over a local HTTP action server so commands can run reliably without fragile focus-based keybinds.

## Included actions

1. Command Palette
2. Quick Open
3. Explorer
4. Search
5. Source Control
6. Extensions
7. Toggle Terminal
8. Problems
9. Save All
10. Format Document
11. Toggle Sidebar
12. Toggle Panel
13. Zen Mode
14. Split Editor
15. New Terminal
16. Stop Debugging
17. Run Task
18. Start Debugging
19. Focus Terminal
20. Open Editor
21. Custom Command
22. Task Wheel
23. Debug Wheel
24. Open Editor Wheel

## Setup

1. Install the **Arclet VS Code Companion** extension.
2. In VS Code open **Settings** and configure:
   - `Arclet VS Code Companion: Port`
   - `Arclet VS Code Companion: Password`
3. Run the Arclet plugin setup and enter the same host, port, and password.

The Arclet plugin then calls the companion with requests like:

`http://127.0.0.1:32145/api/action/vscode.runTask?name=build`

## Query-backed inputs

The plugin avoids manual IDs and uses live queries for:

- tasks
- launch configurations
- terminals
- open editors

