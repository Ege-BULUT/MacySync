# Changelog

Every MacySync release, newest first. The site shows the same list at
https://macysync.vercel.app/changelog/.

## 0.1.1 (2026-11-27)

- The new Mac takes git's `user.name`, `user.email` and credential helper from the old Mac, as an item of
  their own. Each value is set only where the new Mac has none yet, and GOAL.md lists what was set and what
  was kept.
- When the old Mac used gh as its credential helper, the new Mac does not copy that path. If gh is signed in
  there, it runs `gh auth setup-git`; otherwise it asks you to sign in to gh first. Any other helper written
  as a shell command is not copied, because such a command can hold a token. No other credential settings
  are sent.
- The repo has a LICENSE file (MIT), as Cargo.toml and the site already said.

## 0.1.0 (2026-11-27)

First release, for Apple silicon (macOS 13 or later).

- Each Mac gets an id, a name and a look: the real model and colour, detected from the hardware.
- A private GitHub repo as the bus: devices, requests and transfers, with anything private sealed.
- Find the Macs on this account, then pick Send or Receive per session; the other Mac approves.
- 03 PICK: a tree of categories, apps and parts of apps. The receiver picks all or some, and only its picks
  are sent.
- 04 MOVE: a direct Mac to Mac stream, end-to-end encrypted, that resumes after a cut. Progress goes to the
  bus every 2 minutes, with a countdown to the next update.
- MacySync's own Tailscale node in 02 CONNECT, apart from any Tailscale already on the Mac, for Macs that
  are not on the same network.
- Apps move whole with their settings. Each app brings its keychain key (its "Safe Storage" key), and the
  new Mac adds it to its login keychain before the app first starts.
- Links into the old home folder point into the new one after placing.
- Setup brief: AGENTS.md and GOAL.md for the agent you choose, and a preflight that installs Apple's command
  line tools, Homebrew and Node when the new Mac lacks them.
- A mirror-symmetric window cut at 45 degrees, and a layout editor that redraws the running app on save.
