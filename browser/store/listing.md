# Chrome Web Store listing

What goes into each field of the Developer Dashboard for the Chat with Work extension. Keep it in step with `static/manifest.json` and [docs/browser.md](../../docs/browser.md): every claim here must be true of the build that is uploaded.

## Store listing

**Name:** Chat with Work (from the manifest)

**Summary** (the manifest's `description`, at most 132 characters):

> Lets Chat with Work use tabs in its own tab group, on sites you allow, through Chat with Work Desktop.

**Category:** Productivity, Workflow & Planning

**Language:** English

**Description:**

> Chat with Work is an AI assistant for your work. With this extension and Chat with Work Desktop on your computer, it can also use your own browser, with the sign-ins you already have, on the sites you allow.
>
> It works only in its own tab group, "Chat with Work", in the background, so you keep using your other tabs. It reads a page the way a screen reader does, takes a screenshot when it needs to see the layout, scrolls and waits for pages to load.
>
> If you also allow actions, it can open pages on your allowed sites, click, type, choose from lists, close its tabs and save files into a folder you share. Every action asks you first in Chat with Work. Anything that may send, buy, post or delete something shows you a screenshot of the tab with the button outlined before you approve it.
>
> What it never does:
> - Act on a tab outside its tab group, or on a site you didn't allow.
> - Read or type into password, card or one-time code fields. They are blurred in its screenshots too.
> - Run code a website or a server sends it. It does a fixed set of things, the ones above.
>
> Stop in the extension's menu, or in Chat with Work Desktop, ends it at once. The extension and Chat with Work Desktop are open source: https://github.com/crmne/chatwithwork-desktop
>
> Chat with Work Desktop is needed: the extension talks only to it, on your computer, never to a website or to us directly. Install it from https://github.com/crmne/chatwithwork-desktop#install

**Images:** `images/1-tab-group.png`, `images/2-approval.png`, `images/3-stop.png` (1280×800), and `images/promo-small-440x280.png` (the small promo tile). The icon is `static/icons/icon-128.png`. Made by `store/render.mjs` from `store/captures/`.

**Homepage:** https://chatwithwork.com
**Support:** https://support.chatwithwork.com

## Privacy practices

**Single purpose:**

> Lets the Chat with Work assistant read and, with the person's approval, act on web pages in the extension's own tab group, on sites the person allowed in Chat with Work Desktop on their computer.

**Permission justifications:**

| Permission | Why |
|---|---|
| `debugger` | The single purpose. It attaches only to tabs in the "Chat with Work" tab group, on allowed sites, to read the page's accessibility tree, take screenshots, scroll, and click and type for actions the person approved. It detaches after a minute without a call. No script a website or a server sends is run through it: the extension sends only its own fixed commands. |
| `tabs` | Lists the tabs in its group with their titles and addresses, checks each tab's address against the allowed sites before acting, and opens allowed pages in its group. |
| `tabGroups` | Finds and creates its "Chat with Work" tab group, the only place it acts. |
| `nativeMessaging` | Talks to Chat with Work Desktop on the person's computer (`com.chatwithwork.desktop`), its only connection. |
| `storage` | Remembers that the person pressed Stop, so the kill switch survives restarts. |
| `alarms` | Reconnects to Chat with Work Desktop when it starts later than the browser. |
| `downloads` | Cancels and removes downloads a page starts by itself while the extension acts on its tab; files the assistant saves go only into a folder the person shares. |
| Host permission `<all_urls>` | The person chooses which sites to allow, in Chat with Work Desktop, and can allow any site; the extension needs access to read and act on whichever they allow. It refuses every other site itself. |

**Remote code:** No. Everything the extension runs is in the package. Chat with Work Desktop sends it requests by name ("read", "click") with plain arguments; it never sends code, and the extension never evaluates what it receives.

**Data usage**, what to tick:

- **Website content**: text, the accessibility tree, and screenshots of pages in its tab group, on allowed sites.
- **Web history**: the titles and addresses of tabs in its tab group.
- **Personally identifiable information**, **Health information**, **Financial and payment information** and **Personal communications**: none of these is read on its own, but a page the person allows and asks about (a bank, a clinic, a webmail) can show them, and its text or screenshot then carries them.

Not collected: authentication information (it never reads password or one-time code fields, and blurs them in screenshots), card numbers (it never reads card fields), location, user activity (it doesn't watch the person's clicks or keystrokes).

What happens to it: the extension sends it only to Chat with Work Desktop on the same computer, which passes it to the person's own Chat with Work (chatwithwork.com, or their organization's self-hosted server) for the chat that asked. It isn't sold, isn't used for anything but the single purpose, and isn't used for credit decisions.

Certify all three: not sold to third parties; not used or transferred for purposes unrelated to the single purpose; not used to determine creditworthiness.

**Privacy policy:** https://chatwithwork.com/privacy-policy (its "Your browser" paragraph covers the extension, and it must also say that the extension's use of browser data follows the Chrome Web Store User Data Policy: see checklist.md).

## Test instructions for the reviewer

> The extension works with Chat with Work Desktop, which it reaches through native messaging; without it, the extension's menu says "Chat with Work Desktop isn't installed" and does nothing.
>
> 1. Install Chat with Work Desktop (https://github.com/crmne/chatwithwork-desktop#install) and sign in with the test account below.
> 2. In a terminal, run `cww browser enable`, `cww browser allow example.com` and `cww browser allow-changes` (or use the same switches in Chat with Work Desktop's settings).
> 3. Open the extension's menu: it says Ready and lists example.com.
> 4. In Chat with Work, ask: "Open example.com in my browser and tell me what the page says." The assistant asks to open the page (approve it). A tab opens in the background in a group named "Chat with Work", and the assistant reads it.
> 5. Ask it to open a site that isn't allowed, such as wikipedia.org: it is refused.
> 6. Press Stop in the extension's menu: the next request is refused until you press Resume.
>
> Test account: (given in the dashboard's private field)
