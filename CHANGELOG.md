# Changelog

What changed in each release of Working, newest first, written for the people who use it. The app shows this file in its What's New dialog, so it keeps one shape: a `## <version> - <YYYY-MM-DD>` heading for each release, then one `- ` line for each change. A change may start with a short `**title.**` in bold; the rest is plain text, with no other Markdown.

## 0.11.1 - 2026-11-24

- What's New shows each release as a plain list, without a box around every change.

## 0.11.0 - 2026-11-24

- **See who reacted.** Hold the pointer over a reaction under a message to see the people who reacted with it.
- **What's New.** This list of changes opens once after an update, and any time from What's New in the app menu or the version at the bottom of Settings.

## 0.10.1 - 2026-11-24

- **Agents can mention people.** An @all, or an @ followed by a member's full name, in a message an agent sends is a real mention now and notifies the person. Before, it arrived as plain text. A name that two members share mentions neither of them.

## 0.10.0 - 2026-11-24

- **Edit your last message with Up.** Press Up in an empty message box to change the newest message you sent in that conversation. A thread's message box takes your newest reply in that thread. With text or files in the box, Up moves the cursor as before.
- Spaces moved over from Google Chat's older threading show their messages and threads. They showed "No messages yet".
- Rewrite leaves a chat draft alone when it holds only mentions, slash commands, and links, and sends it as it is.

## 0.9.0 - 2026-11-24

- **Rewrite skips drafts that need no rewriting.** A draft that is blank or only links is not rewritten: the Rewrite button does nothing, and Always rewrite sends it as it is. With Always rewrite on, start a draft with ! to send it as written, without the mark.
- The mention menu offers @all in group chats. It listed @all only in named spaces, so a group chat sent it as plain text.

## 0.8.0 - 2026-11-24

- **Agents can save emails as files.** An agent with the Mail tools can save each email of a thread as an HTML page that looks as the email does in Gmail, under who wrote it, to whom, and when, or as an .eml file that is the email exactly as it was sent. Before, an agent read emails only as plain text, so it had nothing to print when asked for a PDF of one.

## 0.7.0 - 2026-11-23

- **One server for agents.** Agents connect to one server, named working, at /mcp, in place of one server for each of Chat, Mail, Calendar, and Meet. The Chat, Mail, Calendar, and Meet switches in Settings pick the tools it offers, so you set an agent up once and it follows the switches. The server's address and port (127.0.0.1:3021 until you change them) are set in a dialog in Settings. The older addresses, one for each product, are gone: set up again any agent that used them.
- **Add to an agent.** Settings copies the setup command for Claude Code, Codex, Grok, Pi, omp, Hermes, OpenClaw, or Antigravity.

## 0.6.0 - 2026-11-23

- **Rewrite reads the conversation.** Rewrite sends the conversation's latest messages with your draft and writes in the conversation's language, unless your instructions or the draft ask for another. A switch in Settings turns this off.
- **A model and an effort for Rewrite.** Each Rewrite tool keeps its own model and effort in Settings. Both start at Harness default, which leaves the choice to the tool.
- Starting a chat with two or more people works. Before, Start chat did nothing for a group.
- A new chat opens at once, before Google answers. A message you send meanwhile goes out as soon as the chat exists.
- A long list item in a message wraps instead of running past the edge.
- Tab stays inside an open dialog, such as Settings, a picker, or the event editor, and no longer reaches what is behind it.
- The Mail labels list opens under its selector, not beside the sidebar. Up and Down move through the labels while it is open.

## 0.5.0 - 2026-11-23

- **About Working.** About Working in the app menu opens a card with the app's version and a link to its repository.
- **Layout and message style pickers.** Layout (Cmd+Shift+L) and Message Style (Cmd+Shift+M) in the app menu list their choices as the color theme picker does: the highlighted one previews in the window, Enter saves it, and Escape puts the saved one back. On Linux and Windows, Ctrl takes the place of Cmd.

## 0.4.1 - 2026-11-22

- **The next thread opens after Move to Trash.** Moving a thread to the trash from the Mail reading pane opens the neighboring thread. Before, it left the pane empty. The pane closes only when the list has no other thread.
- Do not disturb in the IRC list is drawn as it is on an avatar, a red dot with a white bar. A plain red dot read as an unread mark.

## 0.4.0 - 2026-11-22

- **Move a Mail thread to the trash.** The Mail reading pane has a Move to Trash button where its close button was. Escape, and Back in a narrow window, still leave the thread.

## 0.3.1 - 2026-11-22

- When Google stops accepting the sign-in for Gmail or Google Calendar, the app goes to the welcome screen and says why. Before, it stayed open with chats and an error inside Mail, Calendar, and Meet.

## 0.3.0 - 2026-11-21

- **One unread filter in every layout.** Unread is one toggle that combines with the filters for people, groups, spaces, and apps in all four layouts. Each filter shows where the unread conversations are: a count on a Rail tab, on a Google Chat section header, on the Messages toggle, and in the IRC filter menu.
- **Mention marks.** A conversation whose unread messages mention you shows @ in place of the unread dot, also when the mention came while the app was closed.
- **Muted conversations.** A muted conversation shows a mark and counts as unread only once it mentions you, as in Google Chat on the web. It is listed last in its section of the Google Chat sidebar and last of its kind in the IRC list.
- **A New line where the unread messages start.** A conversation you open unread shows a New line above the first message you have not read and opens scrolled to it. The line says when what follows mentions you, and it stays in place until you write. A thread's panel has its own.
- **Counts for new replies and new messages.** A thread's footer says how many of its replies are new (3 replies, 2 new). A message that arrives while you are scrolled up shows a pill over the list with the count: click it, or scroll to the end, to clear it.
- **Changes to the lists.** In the Google Chat layout, the sidebar scrolls to the open conversation, keeps unread and open conversations in sight past Show all, and counts unread on its section headers. In the IRC layout, Mentions, Starred, Calendar, Meet, and Mail are the first rows of the list. In the Rail layout, the first tab is All, and Mentions and Starred take a number key from their tabs.
- **More Mail tools for agents.** An agent you allow can save a new email or a reply in Gmail's Drafts and send it later, forward an email, add Bcc, star threads, change your labels, read attachments, and page through older threads. A change to a thread Gmail does not have is refused, not reported as done.
- A number key that opens Home, Mentions, Starred, Calendar, Meet, or Mail scrolls the list to that view's row, also when the view is already open.
- List columns under 300 pixels wide stay readable: the Messages and Rail headers fit on one line, and rows put the name beside the time and the preview beside the marks.
- The one-line filter opens its menu under the line, not over the label you clicked.
- Scrolling to either end of a conversation loads older or newer messages at once. Before, they loaded one scroll late.
- A message's hover toolbar goes away when the pointer leaves the message, also after a scroll.
- Every tab of the Rail stays whole in a short window. The Calendar, Meet, and Mail buttons under the tabs cut off the last one.
- A message's time keeps up while its conversation stays open. It no longer stays at now.

## 0.2.0 - 2026-11-19

- **Mail.** Gmail in the same window, with the sign-in you already have. It has Inbox, Starred, Sent, All mail, your labels, and Gmail's search, shows HTML emails as they were sent, writes replies and new emails with the signature set in Gmail, archives with Undo, and marks threads read or unread. Rewrite works on a reply or a new email. A switch on an email shows every HTML email on the other background, light or dark. New mail posts a notification and adds to the unread count.
- **Event reminders.** A notification arrives when an event's reminder time comes, as Google Calendar sets it.
- **Features in Settings.** Calendar, Meet, and Mail each have a switch that turns the whole feature off: its view, buttons, notifications, and agent tools. While Mail is off, the app does not read Gmail.
- **A server for each product's agent tools.** Agents connect to a separate server for each of Chat, Mail, Calendar, and Meet, each with its own switch in Settings, under Agents. Sending chat messages, changing events, and sending or changing mail each sit behind one more switch. The single /mcp address is gone: turn a server on and point the agent at that server's address.
- Rewrite no longer returns a short draft with the instructions mixed into it.
- A document attachment opens as a PDF or as plain text only. Before, one that declared itself a web page opened as a page in the preview, scripts included.
- Attachments you open and pictures you paste are kept in a private folder, emptied when the app starts. Before, they went to the system's temporary folder, which other users of a Linux computer can read.
- Only Google's own servers can change the saved sign-in. Before, an answer from any host could add a cookie to it.

## 0.1.9 - 2026-11-17

- **12-hour or 24-hour times, as your system shows them.** Every time in the app follows the system's clock setting: messages, lists, Meet, Calendar, do-not-disturb hours, and the event editor.
- **The IRC list grouped by kind.** The list of the IRC layout keeps spaces, group chats, people, and apps together, each with its pinned conversations first. Group chats start with &, apps with +, and pinned rows with @. An unread conversation shows by its bold name in the accent color.
- **Status in the IRC list.** A person's row ends with their status mark: active, away, or do not disturb.
- Mentions and Starred open at once with what they showed last, then update from the network.

## 0.1.8 - 2026-11-16

- **Less work while you type.** The app redraws only what changed, so typing and moving the pointer over messages no longer rebuild the message list. It starts with less work, holds fewer decoded pictures in memory, and keeps its picture cache on disk to 512 MiB.
- The Meet call window opens on the display of the window you are using.

## 0.1.7 - 2026-11-16

- **Cards show more.** A card from an app shows its header image beside the title and subtitle, each label over its value, and bold text.

## 0.1.6 - 2026-11-16

- **Number keys for any view or conversation.** Cmd+1 to Cmd+9 (Ctrl on Linux and Windows) open what you assign: Home, Mentions, Starred, Calendar, Meet, or a conversation. Right-click a sidebar row, a view button, or a conversation and choose Assign shortcut; a key that is taken says what it replaces. Sidebar rows show their key. The defaults stay Home, Calendar, and Meet.
- A message that is only a Meet link reads "Shared a Meet link" in the sidebar and in notifications, not "Sent an attachment". A message that is only an app's card shows the card's title.
- On macOS, the menu bar's Settings, Color Theme, Find, View, and Go commands stay enabled when the call window is in front.

## 0.1.5 - 2026-11-15

- **The call's chat in the Meet window.** The call controls have a chat button with a count of messages from others. It opens a panel beside the video tiles that lists the call's messages and sends yours: Enter sends, Shift+Enter starts a new line, and Escape closes the panel. A new message shows beside the controls while the call window is in front, and as a notification otherwise.
- The sign-in lasts longer. The app saves the session as Google renews it during chat use, so a restart no longer goes back to the session of the day you signed in.

## 0.1.4 - 2026-11-15

- **IRC layout.** A narrow list of conversation names grouped by kind, a one-line title bar, and a list of members beside spaces.
- **IRC message style.** Time, sender, and text aligned in the monospaced font, without avatars or bubbles. The Density setting is now Message style.
- **Older spaces and group chats out of the way.** The Spaces and group chats setting keeps the ones not active in the period you choose behind a Show older row at the end of the list.
- **Threads in a pop-up.** A thread opens as a pop-up over the window. The Threads setting puts it back in the side panel.
- **Settings in more sections.** Appearance is split into Appearance, Layout, and Messages, and Settings sizes with the window.
- The filter chips wrap onto more lines instead of scrolling sideways.
- Meet calls open signed in when the call window still holds an older sign-in. Before, the call showed Google's sign-in page.

## 0.1.3 - 2026-11-15

- Settings shows the app's version.
- On Linux, an AppImage in a folder you cannot write to skips the update. Before, it downloaded the update every 4 hours.
- On Linux, the old AppImage is no longer left mounted after an update restarts the app.
- On Windows, an update no longer puts back a Desktop shortcut you deleted, and the downloaded installer is removed at the next launch.

## 0.1.2 - 2026-11-14

- A tab of the Rail keeps its label whole in a short window: the tabs keep a minimum height and the list scrolls.
- A menu widens to fit its labels. Before, a long checked item, such as Sort by most recent, was cut.

## 0.1.1 - 2026-11-14

- **Working in your apps menu on Linux.** The AppImage asks once whether to add itself to your apps menu, so the desktop shows Working's icon and not a generic one. A switch in Settings undoes it.

## 0.1.0 - 2026-11-14

- **Working runs on Linux and Windows.** Linux gets an AppImage for x64 and Arm64, with system notifications and color emoji; meetings open in your browser there. Windows gets an installer for x64 and Arm64, with system notifications. On both, one menu button in the top bar takes the place of the menu bar.
