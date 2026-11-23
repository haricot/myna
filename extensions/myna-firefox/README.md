# Myna Firefox Navigation R1 (No LLM)

This copies the tested native **dictation anchor** from `feature/myna-firefox-native-anchor`
without changing its v1 Native Messaging protocol, host, background.js or content.js.
The R1 browser navigation capability is a **separate Firefox sidebar** with its own
strict, deterministic natural-language router. Current branch:
`feature/myna-candle-native-anchors-r1`.

## Running on Niri + Firefox (test profile recommended)

1. `git pull --ff-only` in `~/tmp/myna-candle-client-native`.
2. In `about:debugging#/runtime/this-firefox`, **remove the old temporary Myna add-on**
   and load `extensions/myna-firefox/manifest.json` from this checkout.
   It uses the same extension ID `firefox-anchor@myna.local`; do not run two copies.
3. Open Firefox Sidebars → **Myna Navigation**, or assign the Firefox command
   `_execute_sidebar_action` a shortcut in about:addons → Manage Extension Shortcuts.
   The toolbar button retains its original dictation-anchor meaning.
4. Click **Autoriser ce site** (explicit Firefox host permission for the current site).
   The sidebar will not request permissions on a voice-only event.
5. Focus the sidebar command field, dictate with the existing Myna shortcut,
   then press Enter or click Exécuter. For example:
   - `affiche les liens`
   - `clique sur Rechercher`
   - `ouvre le troisième lien` (requires a recent `affiche les liens`)
   - `attends que le bouton Continuer apparaisse`
   - `dès que le bouton Continuer apparaît, clique dessus`
   - `descends dans la page`

**Voice boundary status:** Myna provides the voice input using its already working
IBus text insertion into the sidebar field. **Automatic direct transcript delivery
from Myna to the browser route without focusing the sidebar is not yet connected.**
That next step requires an explicitly armed command session, not widening the
current private `myna-firefox-host` protocol silently.

## Fixed protocol for future decision providers

`MynaIntentRouter.route(text)` returns `{version:1,status:"ready",action}`
(or `status:"unmatched"`). Only the strict action vocabulary
`list`, `scroll`, `click`, `wait`, `wait_click` is allowed.
The extension validates every action. A future Liquid D1 / Mistral decision
provider can select among externally prepared *validated* candidates; it
must not generate arbitrary JavaScript, CSS selectors, URLs or shell commands.
No LLM or inference dependency is required for R1.

## Security and limitations

- No `<all_urls>` install grant, no accessibility service and no vision GPU.
- No page contents or full DOM snapshots are forwarded to Myna or the native host.
- Maximum 60 labeled visible elements per listing; indices expire after 60s or navigation.
- Only active-tab HTTP(S) documents with an explicit site grant; no frames,
  protected pages, browser chrome or file URLs.
- A click cannot submit forms, download files, visit other origins, or target
  obviously destructive/payment actions. Some legitimate site buttons will be
  refused; this is intentional for R1.
- `MutationObserver` waits a maximum of 8s (12s hard upper limit), and
  rejects a changed or hidden tab. A separate cancel button is provided.
- Programmatic `.click()` is not a trusted user gesture; some sites may ignore
  it. No universal support for closed Shadow DOM or custom canvas controls.
- These changes must be reviewed/tested in an actual Firefox session before
  treating them as production-ready.

## Tests

```sh
cd extensions/myna-firefox
node --test test/intent-router.test.cjs
npm ci && npm test
```

Do not enable any new native host capabilities for this R1. To use only the
original anchor and not the navigation sidebar, simply don't grant navigation
permissions or open the sidebar.
