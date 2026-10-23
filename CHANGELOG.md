# Changelog

Every release of luxe, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While luxe is below
1.0, a minor bump (0.x.0) may break config, flags or the `--rpc` protocol, and a patch bump never
does.

The version on `main` is always the next release. The first change to what ships (`crates/`,
`Cargo.toml`, `Cargo.lock`) after a release bumps it one step and opens its section here, and later
changes add to that section until it ships; `scripts/version-check.py` holds them to it.

## [0.1.6]

### Added

- On macOS, `screenshot` with `mode=window` captures one window, even one on another desktop. It
  matches the window's title first, then its app's name, and with no title it takes the frontmost
  window. A miss lists the windows it saw.

### Fixed

- On Windows, a pasted line break no longer sends the message. The console types a paste out one
  key at a time, so luxe gathers a burst of keys back into one paste.
- On a scaled Windows display, `screenshot` captures the whole screen instead of its top-left
  corner.
- On a Mac with fontconfig installed, the OpenAI and Claude marks fall back to the robot and cloud
  when no font has them, instead of drawing a box with a question mark.

### Security

- `clipboard-win` 5.4.3, since earlier releases were yanked: reading an HTML fragment with a bad
  offset from the Windows clipboard could read past the end of a buffer. luxe reads only text and
  images from the clipboard, so it never reached that code.

## [0.1.5]

### Added

- Clicking the provider in the dock lists your providers. Choose one that serves the model you're
  on and luxe switches to it there, keeping the model; choose any other and its models open. In
  the model picker, `←` reaches the same list.
- The guided review's pull request brief opens with the start of the description, then **Read the
  whole description · p**. `p` shows all of it.
- The FILE tab of the preview shows a file the session changed outside the project, highlighted
  like any other. It reads only what the session itself changed out there.

### Changed

- The model picker opens on the models of the provider you're using, and `tab` shows every
  provider's. Signing in opens it on the provider you just signed in to.
- Models go by their ID everywhere, like `claude-opus-5`, unless you name one in `models.json`.
- The dock's model row wears the maker's mark (Claude's for `claude-*`) and the provider row who
  serves it. A provider with no mark of its own, Anthropic among them, gets a filled cloud, and a
  ChatGPT subscription wears OpenAI's.
- `luxe update` runs the same installer you first installed with, so an update lands where the
  install did. The separate `luxe-cli-update` program is gone.
- The mode sits above the composer.
- The guided review's guide asks the model for less, and says less.
- Commits from a guided review carry your message and nothing else. The `Luxe-review` and
  `Luxe-revision` lines are gone.
- The guided review no longer flags CI and supply-chain files as "weakened gates".
- The start screen's launcher is ruled off with a muted line inside its frame.

### Fixed

- A one-line `bash` command on a tool card's header is highlighted, like the rest of the card.
- The preview runs down to the activity row when it takes the conversation's place, without a
  blank row between them.
- The dock's provider and model rows light up and take clicks separately.
- In the phone app added to an iPhone's Home Screen, iOS no longer blurs the top bar.

### Security

- `rustls-webpki` 0.103.16 and `webpki-roots` 1.0.10, for RUSTSEC-2026-0381 and RUSTSEC-2026-0382:
  certificate checks could trust distrusted intermediates.

## [0.1.4]

### Added

- luxe remembers the model you last chose in each project, and starts there. A project you haven't
  chosen in yet starts on the model you last chose anywhere, then on `default_model`.
- The mouse wheel and the scrollbar work in every list in a modal: the model picker, the palette,
  themes, sessions, providers, settings and the `/` and `@` flyouts. Both move the cursor, so Enter
  never picks a row you can't see.
- The dock's telemetry row starts with the provider, beside its own mark where the font has one.

### Changed

- The name is written lowercase everywhere, as **luxe**, even to start a sentence. The dot in
  **luxe.** belongs to the logo.
- `luxe --help` opens with "A coding agent for your terminal."
- Signing in no longer picks a model for you. The model picker opens at the top of its list, and
  nothing changes until you choose.
- Loop output marks each phase with a colored rail and each iteration with a bracket inside it. In
  a terminal a tool's line shows `○` while it runs, then a green `✓` or a red `✗`.
- The phone app shows a turn's reasoning in full, as the interface does. It was cut to three lines
  with nothing to say there was more.
- A phone notification for a question reads "The agent has a question for you".

### Fixed

- On Windows, `luxe update` replaces the running luxe. Windows won't overwrite a program while
  it runs, so the old copy is moved aside and deleted at the next launch.
- A luxe that's already signed in no longer opens first-run setup, even if setup was never
  finished. Quitting it partway left nothing saved, so it came back at the next launch.
- Opening a large project in the file tree no longer shows "engine busy". It asked for every
  changed folder at once and overflowed the queue; it now asks for a few at a time.
- The phone app stays where you are while you read back through a long session. Each new item
  dropped the oldest from the top of the page, which moved everything you were reading.
- A question answered from the phone app shows its answer on its card, as the interface does. It
  was hidden behind a tap, as raw text.

## [0.1.3]

### Added

- A session open in the interface can be joined by another program on the same machine while you
  keep working: `luxe join` speaks the [RPC protocol](docs/guide/rpc.md#join-a-running-session)
  over a local socket (a named pipe on Windows) only your account can open. A joined client gets
  the session so far, then every live event, and can answer prompts. It can't switch or end the
  session, publish to GitHub, or be a program the agent started. `"joinable": false` or
  `LUXE_JOINABLE=0` turns it off.
- A `prompt_closed` event tells every front end when a permission prompt or question stops
  waiting, so one answered elsewhere comes down everywhere.
- `luxe remote` puts the sessions running on your computer in reach of your own phone or tablet,
  over [Tailscale](docs/guide/remote.md). It listens on this machine only, Tailscale Serve carries
  it to your devices, and it lets in your own Tailscale login from a device you paired with a QR
  code. `luxe remote setup` checks Tailscale, publishes the bridge, runs it at sign-in as a user
  service (systemd, launchd, or the Windows Run key, none needing an administrator), and shows the
  code. `luxe remote devices` lists and revokes devices.
- The bridge can start Luxe in a folder a device picks: in a herdr pane when herdr is running, so
  the session waits at your desk, or as its own `luxe --rpc` otherwise.
- A session started with `luxe --rpc` can be joined, like one in the interface.
- An `introduce` action lets a joined client name itself, so the interface says "iPhone joined
  this session" and "Approved from iPhone" rather than a process id.
- The bridge renders messages for the phone app the way the interface draws them: markdown,
  highlighted code in your theme's colors with a copy control, tool icons, and your own messages
  without the notes Luxe adds for the agent. The phone parses nothing itself.
- From the phone app you can write to a session, steer or stop a running turn, and answer its
  permission prompts and questions. Like the interface's, a prompt holds every answer but Reject
  for a moment after it appears, so a tap meant for something else can't grant anything.
- The phone app's status line shows the mode behind the interface's mark for it, which a tap
  switches, and the context, thinking level and model, worded as the interface's status bar words
  them. A tap on those opens the session's details, where you can switch the model or thinking
  level and see the cost.
- The phone app is set in the same monospace at a size that fits more on a phone, and its session
  list wears the Luxe wordmark, lit by the same slow drift as the interface's.
- The phone app's header names the session's folder and the branch it's on, behind the interface's
  folder and branch marks, starred when the tree has changes. A long branch is cut first.
- The phone app can start a session in a folder you've worked in, or any other you type, and opens
  it once it's ready. **New** on the session list opens a page for it, where what you type narrows
  the folders it offers.
- The phone app can send photos, from the camera or the library, scaled down on the phone first so
  they're quick to send.
- The phone app's composer has icon buttons for attaching a photo (a paperclip), sending (an arrow
  in the accent color, dim until there's something to send) and stopping, and a line over the box
  while a turn runs that says whether sending steers it or waits for it to end.
- The phone app offers the slash commands it can carry out (`/compact`, `/cost`, `/dock`, `/mode`,
  `/model` and `/thinking`) as you type a `/`, and project files to mention as you type an `@`.
- The phone app shows the Context Dock, with the session's plan and changed files: a rail beside
  the conversation on an iPad or an open iPhone Duo, on the side your `dock` settings choose,
  which a header button and `/dock` show and hide. On a phone the header keeps to one menu button,
  and the dock is the first tab of the panel it opens. A long plan scrolls in a box of its own
  that keeps the work in hand in view, as the interface's rail does. Tapping a file opens its diff.
- The phone app has a project panel, behind the header's menu button. Its Files tab is the
  interface's FILES tree, with its guides, icons and change marks, and shows a file highlighted
  as the interface shows it, with a button that mentions it in your message. On a wide screen the
  panel is a rail in the dock's place, and a file or a diff opens beside it, with the conversation
  still in view where there's room, as the interface peeks beside its dock.
- The phone app's project panel has a GitHub tab: `/github`'s sections and rows, and a pull
  request or issue opened beside them with its overview and conversation, drawn as the interface
  draws them. It reads only, and **Mention** adds the item to your message.
- The phone app's project panel has a Review tab: the open Guided Review's guide, a chapter opened
  beside it with the guide's notes over its diffs, and **Reviewed** and **Next** to walk it. It
  joins the review the interface has open, through the new `show_open_review` RPC action, whose
  answer also says what each chapter asks of the reader. The panel's bar names the project and its
  branch with the interface's marks, as the interface's PROJECT section does.
- The phone app asks about picked lines, as the interface's File view does, in a file, a diff or a
  review chapter: tap line numbers to pick them, or select text the phone's way, down to part of a
  line, and **Ask** adds the path and lines to your message in the interface's words.
- The phone app comments on a review chapter: on a picked line or run of lines, or on the chapter,
  as a comment or a question, saved or sent at once. Comments show under what they're about with
  the agent's replies, and take a reply, resolve, send or delete; the guide sends everything unsent.
- The phone app is named for the computer it reaches, as Tailscale names it: on the Home Screen,
  beside the Luxe mark in the session list, and after the project in notifications. One phone can
  keep an app for each of several computers and tell them apart.
- Your own phone joining a session no longer adds "joined" and "left" lines to the conversation
  each time it reconnects: the dock's SESSION block names who's joined instead. Someone else's
  device is still announced.
- Escape on an iPad keyboard closes the phone app's sheet straight after a pane opens or closes,
  where it used to be lost for a frame.
- The phone app's icon buttons (close, project, dock) are square, 44 points to tap, and a press or
  a keyboard's focus shows Luxe's own square rather than the browser's rounded highlight.
- The phone app's headings are the interface's badges (PROJECT, PLAN, FILES, MODEL, SESSION), cut
  the same way at the same depths, or bold words when `statusline.style` is flat.
- The phone app can be added to the home screen, where it opens full screen with the Luxe mark in
  your theme's colors as its icon. An app on the home screen keeps its own sign-in apart from
  Safari's, so one that needs pairing again takes the invitation link pasted in.
- The phone app can [notify you](docs/guide/remote.md#notifications) when a session waits on an
  answer or finishes a long turn. It's off until you turn it on, one device at a time, and a
  notification says only what's waiting and where. The bridge seals each one for your device and
  posts it to your device's own push service, with no Luxe server in between.
- The protocol sends [`turn_started`](docs/guide/rpc.md) when a turn begins, from any client, and a
  client joining mid-turn gets it while catching up. A turn sent from your phone shows as running
  in the terminal right away, not from its first word.
- The protocol answers [`list_models`](docs/guide/rpc.md) with the models you can switch to, and
  the thinking levels each one honors, so a client can offer the same choices as `/model`.
- A joined client can introduce itself with `watch_only`, to read a session without the interface
  announcing it. Luxe drops anything else it sends. The bridge watches for notifications this way.
- An `AGENTS.md` in a folder the agent works in now reaches it, even when Luxe started somewhere
  else in the repository. It's added to the first tool result that reaches the folder, once, so
  the system prompt and the provider's cache are untouched. Luxe shows "Following" and the file.
  See [Project context](docs/guide/project-context.md#folders-the-agent-works-in);
  `LUXE_NESTED_INSTRUCTIONS=0` turns it off.

### Changed

- A session is open in one Luxe at a time. Resuming one that another Luxe has open is refused, and
  the message says which process has it, rather than two of them writing the same history.
- `message_committed` now announces the messages you send too, as the protocol guide always said
  it did, so a joined client sees the question as well as the answer, and the interface shows a
  message a joined client sent. A `--rpc` client gets its own message back once Luxe saves it.
- The turn timer reads like a tool card's time: `42s`, then `3m 07s`, then `1h 05m`.
- `luxe --help` opens with "A beautiful coding agent for your terminal."

### Fixed

- A tool call the model writes while an earlier one is still running shows as waiting, not as a
  second spinner. Only one card spins at a time again.
- A running `bash` card is titled with what the command is for, as soon as the model has written
  that. It used to show the command in both the title and the body until the command finished.
- A message with an image, sent while a turn runs, is queued for its own turn. Steering it
  delivered the words and dropped the image.
- A compaction is in the conversation from the moment it starts ("Compacting the conversation —
  579.4k of context…"), and that line becomes the result when it ends. A minute of summary showed
  nothing but the activity bar while the dock was open. The phone app shows it too.
- Typing in the phone app keeps up in a long session. It drew every item the session ever had,
  and Safari slows each keystroke as a page passes a few thousand (a 9,000-item session took about
  100ms a key on a desktop). It now draws the latest 300, with **Show earlier** for the rest.
- A long session no longer stalls before every model call. Luxe re-read and parsed the whole
  session file each time, about 70ms on a 68 MB session. It now keeps the session it's writing in
  memory and reads only what was added, about 10ms.
- A turn no longer spends seconds saving the project for **Last Turn** review. Every turn saved
  two full copies of every file, synced to disk one by one (about 11 seconds a turn in a project
  of 4,000 files), and never deleted any, so one project's `.luxe/reviews/turn-baselines` reached
  18 GB. A copy now links the files the last one already holds, which takes under a second, and
  copies the last turn doesn't need are deleted once they're an hour old.
- A long session's memory climbs much more slowly. Every model call copied the whole
  conversation, every tool result included, to send the part after the last compaction. It now
  copies only what it sends: over 20 turns on a 68 MB session, Luxe grew by 65 MB instead of
  338 MB.
- Drawing a long session with screenshots in it is cheaper while a reply streams. Every tool
  card holding an image was laid out again on every frame, even scrolled far out of view, which
  in a session with a hundred screenshots was most of what the terminal UI allocated. A card is
  now laid out again only when its image changes.
- A phone joining a long session costs the computer less. Each join copied the whole
  conversation, then built the message carrying it in one buffer that doubled as it grew. It's
  now sent from the copy Luxe already keeps, in pieces: over 120 joins of a 68 MB session, 26 GB
  less allocated, and each join about 20ms faster.
- Compaction no longer fails more often the longer a session runs. The summarizer was asked for a
  list of every file the session touched and shown the last summary's list to copy, though Luxe
  keeps that list itself and replaced whatever the summarizer wrote. On a long session the list
  grew to over 500 paths, and copying it used up the summary's output limit, so the compaction
  failed with "the summary stopped short". The summarizer now writes only the prose, and its
  output limit is 32K tokens, up from 16K.
- `web_search` with several phrasings gives each its share of the results. Without a key, they
  went to Exa as one search ranked against the first phrasing, so the others rarely showed. Each
  phrasing is now its own search, and the results take turns: the top of each, then the next.
- `web_search` says when Exa's free search is rate limited, and which phrasing it turned away. The
  notice used to read as a search that found nothing.
- A `grep` glob starting with `!` excludes, as `rg -g '!target/**'` does. It used to match no file,
  so the search came back empty.

## [0.1.2]

### Fixed

- Compaction works with thinking models. Summaries were cut off at 4,096 output tokens and thrown
  away, so after three failures automatic compaction switched itself off and the window filled
  until the model could not answer. `/compact` now gives the real reason when it fails.
- On Windows, a timed-out or cancelled command ends everything it started, not just its shell, and
  the agent's commands no longer rename your terminal tab.
- On Windows, the file tools accept Git Bash paths (`/c/Users/...`), `glob` reads `src\*.py` as a
  folder, and the agent's Python commands run in UTF-8 mode, so printing characters outside your
  code page no longer crashes them.
- On Windows, an npm tool installed as a `.cmd` shim (such as `pyright`) is found, and Python is
  looked up as `py -3`, then `python`.
- The debugger checks that `debugpy` is installed before offering Python debugging, and
  `luxe doctor` says how to install it.
- A new session started from the start screen keeps its working indicator and timer from the first
  moment.

### Changed

- The dock shows "compacting" while a summary is written, and its context reading drops as soon as
  it finishes. `--rpc` clients get the new `compaction_started` and `compaction_stopped` events.
- An empty reply from the model at a nearly full context window suggests `/compact` instead of a
  retry.
- Signing in with GitHub: the choices are clickable, and the device code has a Copy button (or
  press `c`).
- The installer's closing message says how to start Luxe.

## [0.1.1]

### Fixed

- Editing a file with the edit or write tool, or format-on-save rewriting it, keeps the file's
  permissions, so an edited script stays executable. A kept mode is now exact rather than trimmed
  by the umask.
- The changes pane's header keeps its position (`8 of 14`) whole, shortening the path instead, and
  keeps a gap between the line counts and the FILE / DIFF switch.

### Changed

- `grep` names each file once with its matching lines under it, rather than repeating the path on
  every line: 9–28% fewer bytes than `rg -n` on the searches measured. `scopes: true` also labels
  each run of matches with the function or type around it.
- The install guide leads with `curl -fsSL https://luxe.dev/install.sh | sh` and
  `powershell -c "irm https://luxe.dev/install.ps1 | iex"`.

## [0.1.0]

The first public release.
