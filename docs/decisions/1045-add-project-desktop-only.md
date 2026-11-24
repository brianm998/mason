# 1045: Add Project is a desktop-only wizard over the onboarding endpoints

The Swift monitor adds a File menu item and a + under the project picker that open a wizard driven by `GET /onboarding/candidates`, `POST /onboarding/plan` and `POST /onboarding/apply`. The wizard state is plain `MonitorCore` code so tests need no view. The sheet, the + and the menu item are compiled for macOS only, because the iOS app shares `MonitorUI`, and are enabled only for the owner on a connection that is not read-only.

The landing choice (no, merge, push, pr; default no) and the proof-task toggle (default on) are asked on Review and sent only with the apply, never with a plan, so a plan can never land anything. The landing answer is the owner's explicit choice, as decision 0290 requires.
