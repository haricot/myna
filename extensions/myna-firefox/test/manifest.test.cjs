const test = require("node:test");
const assert = require("node:assert/strict");
const manifest = require("../manifest.json");

test("the Firefox bridge has only narrow, explicit permissions", () => {
  assert.deepEqual(manifest.permissions, ["activeTab", "scripting", "nativeMessaging"]);
  assert.equal(manifest.host_permissions, undefined);
  assert.deepEqual(manifest.optional_host_permissions, ["https://*/*", "http://*/*"]);
  assert.equal(manifest.browser_specific_settings.gecko.id, "firefox-anchor@myna.local");
});

test("the user can assign a focus-preserving arm shortcut", () => {
  assert.equal(manifest.manifest_version, 3);
  assert.ok(manifest.commands._execute_action);
  assert.equal(manifest.commands._execute_action.suggested_key, undefined);
});
