# Browser site information — 2026-10-16

## Behavior and scope

The site-information popover contains two entries: connection details and Cookie/
website data. Copy URL and external-browser actions are absent from this popover.
Website permissions that the application cannot control are not presented.

The native UI check used an application clone, a temporary local Server/database
and a seeded conversation. No real task was opened and no Agent prompt was sent.
The earlier release `b816197` was checked for the menu/page retention fix. The new
site-data UI was checked with a debug build using `custom-protocol` and built web
assets. This is separate from package/signature checks and the native store test.

## Observed native UI

- A local page with a declared `/brand.svg` and failing `/favicon.ico` displayed its
  blue site icon in the tab and site-information header.
- The browser menu retained the page image in split and merged workspaces. Escape
  restored the native page accessibility subtree and returned focus to the trigger.
- HTTP showed an unencrypted connection; GitHub showed a secure connection, its
  real site icon, `github.com` certificate subject and the native chain issuer
  `Sectigo Public Server Authentication CA DV E36`.
- The final home panel showed only connection and website-data entries. The data
  page showed actual Cookie/cache categories and the native domain scope. Opening
  clear displayed the shared Modal, focused Cancel, made the background inert and
  described logout/subdomain effects. Cancel and Escape preserved the page.
- The actual destructive UI button was not activated against visited browser data.
  Actual deletion and scope isolation were verified in the separate ephemeral
  native-store test below.

Local captures are in the worktree's ignored `target/` directory:
`browser-menu-native-verified.png`, `site-info-main-native.png`,
`site-info-certificate-native.png`, `site-info-final-native.png` and
`site-info-final-native-ax.txt`. These are local evidence, not repository assets.

## Native store and regression checks

`python3 scripts/test-site-data.py` compiles the production Objective-C implementation
with warnings denied and tests a real ephemeral WebKit cookie store. It verifies
root/shared-subdomain selection, unrelated-domain preservation, no cookie-value
exposure, navigation rejection, token consumption/replay rejection and expiration.
WebKit record removal alone left session cookies in this test; the implementation
therefore explicitly deletes matching HTTP cookies before acknowledging completion.

Frontend tests cover metadata loading, connection detail navigation, absence of the
removed actions, empty/stale data, cancellation, confirmation, double-click exclusion,
fresh reads after failure and the existing native-preview lifecycle. The final run
passed 612 frontend tests; desktop unit tests, Clippy, typecheck, formatting and web
build are checked separately. No claim is made for managing camera, microphone,
local-network or application permissions, or for non-macOS website-data support.
