# IntelliJ IDE Plugin for KnobDeck

KnobDeck companion plugin for IntelliJ-based IDEs. It mirrors the JetBrains Stream Deck plugin action surface and talks to the **KnobDeck IntelliJ Companion** IDE plugin over a local HTTP action server.

## Included actions

1. Open Action Browser
2. Customized Action
3. Update Project
4. New Project
5. Run
6. Debug
7. Step Over
8. Step Into
9. Step Out
10. Resume
11. Pause
12. Search Everywhere
13. Stop
14. Project Structure
15. Reset Frame
16. Terminal
17. Claude Code

## Setup

1. Install the **KnobDeck IntelliJ Companion** plugin into IntelliJ IDEA / Android Studio / WebStorm / other JetBrains IDEs.
2. In the IDE open **Settings → Tools → KnobDeck**.
3. Enable the action service and choose the port/password.
4. Run the KnobDeck plugin setup and enter the same host/port/password.

The KnobDeck plugin then calls the companion with requests like:

`http://127.0.0.1:21420/api/action/SearchEverywhere`

Run and Debug actions also support an optional run-configuration name via `?name=...`.
