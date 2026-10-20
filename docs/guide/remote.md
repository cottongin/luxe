---
title: "luxe on your phone"
summary: "Reach the sessions running on your computer from your own phone or tablet, over Tailscale."
order: 13
group: "Using luxe"
---

`luxe remote` lets your phone or tablet see the sessions running on your computer, join one, and
start new ones. It listens on your computer only. [Tailscale](https://tailscale.com), a private
network between your own devices, carries it to them over HTTPS, so nothing is opened to the
internet and there is no luxe server in between.

On the phone it's an app in the browser, drawn like the interface in your theme. For the whole
terminal, editor included, [luxe from your phone](recipes.md#luxe-from-your-phone) works over SSH.

![On an iPhone, an edit waiting for permission](phone/remote-permission.png)
![The same turn finished: its tool cards and the reply](phone/remote-turn.png)

## Set it up

1. Install Tailscale on the computer and on the phone, and sign in to the same account on both.
2. On the computer, run:

   ```bash
   luxe remote setup
   ```

3. Scan the QR code it shows with the phone's camera, and give the device a name.
4. On an iPhone or iPad, add it to the Home Screen: in Safari, Share, then Add to Home Screen. It
   opens full screen with the luxe mark as its icon, and keeps the sign-in Safari had when you
   added it. An app added before pairing asks for the invitation link instead: copy it from
   `luxe remote pair` and paste it in.

Setup takes four steps, and asks before each one that changes something outside luxe:

| Step | Does |
| --- | --- |
| Tailscale | Checks it is installed and signed in, and stops here if not |
| Serve | Publishes the bridge at `https://<computer>.<tailnet>.ts.net/` with Tailscale Serve, for your tailnet only. One that already leads somewhere else is left alone. |
| Service | Runs the bridge whenever you're signed in to the computer |
| Pairing | Shows a QR code for one device |

A step that's already done says so and moves on, so running setup again is how to check it. On
Linux, Serve may say access was denied; let your account manage Tailscale once, then run setup
again:

```bash
sudo tailscale set --operator=$USER
```

The service is your own, and needs no administrator:

| | |
| --- | --- |
| Linux | A systemd user unit, `~/.config/systemd/user/luxe-remote.service` |
| macOS | A launch agent, `~/Library/LaunchAgents/dev.luxe.remote.plist` |
| Windows | `luxe remote` under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |

Without it, run `luxe remote` in a terminal, and your devices reach luxe while it runs.

## Devices

```bash
luxe remote pair                    # a QR code and link for one more device
luxe remote devices                 # the paired ones
luxe remote devices revoke a41c9e02 # unpair one
```

An invitation pairs one device, within ten minutes. Revoking a device disconnects it within a few
seconds; the sessions it had open carry on.

Only your own Tailscale login gets in. To let someone else on your tailnet in too, name them when
you set up, and send them an invitation from `luxe remote pair`:

```bash
luxe remote service remove
luxe remote setup --allow sam@github
```

## Two or more computers

Each computer runs its own bridge at its own address, so one phone can reach several: set each up
as above, with Tailscale signed in to the same account, and pair the phone with each. Every one is
its own app on the Home Screen, named for the computer as Tailscale names it (`desk` for
`desk.tail1234.ts.net`), with the name beside the luxe mark in its session list and after the
project in its notifications, "luxe · desk". luxe finds Tailscale by the `tailscale` command, so
on a Mac check that `tailscale version` works in Terminal before running setup.

## What your device sees

Every luxe running on the computer as you: the interface, `luxe --rpc`, and any started from your
device. A device that joins a session is a guest, as in [joining a
session](rpc.md#join-a-running-session): it sees the conversation so far and every live event, and
can answer prompts and send messages. It can't switch or end the session, or publish to GitHub.

In a session, the app can:

- Name the session's folder and branch in the header, starred when the tree has changes.
- Show the conversation as the interface draws it: markdown, highlighted code, tool cards. A long
  one opens on its latest 300 items, and **Show earlier** at the top brings 300 more at a time.
- Send a message, with photos from the camera or library, and steer or stop a running turn. While
  one runs, a line over the message box says what sending will do: steer it at the next step, or
  wait for the turn to end when a photo is attached.
- Run `/compact`, `/cost`, `/dock`, `/mode`, `/model` and `/thinking`, offered as you type a `/`. The
  interface's other commands work its own screen, so a message that starts with one goes to the
  agent as written.
- Mention a project file: type `@` and pick from the project's folders, a level at a time. A picked
  file goes in as its path, which is what the interface sends the model for one.
- Answer permission prompts and questions. As in the interface, every answer but Reject waits a
  moment after a prompt appears, so a stray tap can't grant anything.
- Switch the mode with a tap on the status line, which also shows the context, thinking level and
  model. Tap those for the session's details: switch the mode, the thinking level or the model
  (from the same list as `/model`, with the levels each model honors), and see the cost.
- Show [the dock](interface.md#the-dock), with the plan and the changed files. On an iPad or an
  open iPhone Duo it's a rail beside the conversation: the header's dock button or `/dock` shows
  and hides it, `/dock left` or `/dock right` moves it, and it starts from your `dock.mode` and
  `dock.position` [settings](configuration.md), as the interface does. On a phone the header has
  one menu button, and the dock is the first tab of the panel it opens; `/dock` opens it there
  too. A long plan scrolls in a box of its own that keeps the work in hand in view, until you
  scroll it yourself. Tap a file for its diff.
- Browse the project's files from the header's menu button, in the interface's FILES tree, and
  read a file highlighted as the interface shows it. **Mention** adds its path to your message.
  On an iPad or an open iPhone Duo the tree is a rail and a file opens beside it; on a screen wide
  enough the conversation stays in view too.
- Ask about particular lines of a file, a diff or a review chapter, as you would in the interface's
  File view: tap a line's number to pick it and another to stretch the pick, or select text the
  phone's usual way, down to part of a line, and tap **Ask** for the lines it covers. Your message
  gets the path and lines the interface would send. Tap anywhere else to let a pick go, and copy a
  selection with the phone's own menu. In a diff, a removed line can't be picked, since the file
  no longer has it.
- Read the project's pull requests and issues in the same panel's GitHub tab, by the sections
  [`/github`](github.md) shows. Tap one for its overview (what stands between it and merged, or
  who's on an issue) and its conversation. **Mention** adds it to your message. It only reads: a
  joined session runs with GitHub writes off, so merging, replying and resolving stay in the
  interface.
- Walk the open [Guided Review](guided-review.md) in the Review tab: the guide's summary and
  chapters, each one opened beside it with the guide's notes over its diffs. **Reviewed** marks a
  chapter read, the same progress `/review` keeps, and **Next** opens the one after. The phone
  joins the review the interface has open rather than starting its own, since a session holds one;
  with none open, **Review changes** reviews the working changes.
- Comment on a review chapter: pick a line, or a run of lines, and tap **Comment**, or tap
  **Comment on the chapter** with nothing picked. Make it a question if you want an answer rather
  than a fix, then **Save** it to send later or **Send** it now. Each comment shows under what it's
  about with the agent's replies, and can be replied to, resolved, or, unsent, sent or deleted.
  The guide counts what's unsent and sends it all. Comments made in the interface show up too.
  Editing a comment, re-anchoring one the code moved away from, and commenting on a removed line
  are the interface's.

The interface says which device joined, "iPhone joined this session", and which one answered a
prompt, "Approved from iPhone". A session with [`joinable`](configuration.md#joining-a-session) set
to `false` is out of reach, from your device too.

## Starting a session

Tap **New** on the session list to start luxe in a folder: one where luxe is running now, one with
a [herdr](recipes.md#run-luxe-in-herdr) pane, one where the bridge has seen luxe run, or any path
you type. What you type narrows the folders it offers, by name and then by path. They're only the
ones above: it never looks through your home folder.

**herdr is optional, and recommended.** You don't need it to reach your sessions; it decides what
happens to the ones you start away from your desk:

- **With herdr running**, the bridge opens a herdr pane in that folder, in the workspace already
  there or a new one named after it. The session survives a restart of the bridge, and waits for
  you at your desk like any other pane.
- **Without it**, the bridge runs `luxe --rpc` itself, and the session lasts as long as the bridge
  does. At your desk, `luxe --resume` on it is refused while the bridge has it, and says so.

The bridge uses herdr only when herdr's server is already running, and never starts it.

## Notifications

A device can hear when a session needs it: a prompt waiting on an answer, or a long turn finished.
Turn them on under **Notifications** in the app's session list. On an iPhone or iPad the app has to
be on your Home Screen first (in Safari, Share, then Add to Home Screen), and paired from there, since
the Home Screen app keeps its own sign-in.

A notification says what's waiting and where, "Approve bash" over `my-app`, never the command, and
tapping it opens the session. Answering takes the app. So that one doesn't buzz you while you're
at your desk:

- A prompt is held for ten seconds, and one answered meanwhile is never sent.
- A finished turn is news only after a turn of thirty seconds or more.
- A device with that session open on screen isn't told about it.

While any device has notifications on, the bridge joins each running session to watch. It joins
watch-only, so the interface doesn't announce it and it can't act in the session.
[Security](security.md#luxe-remote) covers what a notification carries and where it goes.

## Turn it off

```bash
luxe remote service remove
tailscale serve --https=443 off
```

The first stops the bridge and stops it starting at sign-in; the second takes the address off
your tailnet. Your paired devices stay paired until you revoke them, in case you set it up again.

## How it's kept to you

Every request needs your Tailscale login, which only Tailscale Serve can vouch for, and every
session needs a paired device as well. [Security](security.md#luxe-remote) has the details.
