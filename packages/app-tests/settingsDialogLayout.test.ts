import { readFileSync } from "node:fs";
import { strict as assert } from "node:assert";
import test from "node:test";

const source = readFileSync("apps/desktop/src/components/editor/EditorSettingsDialog.vue", "utf8");

test("settings dialog uses a side category navigation", () => {
  assert.match(source, /settingsCategoryNav/);
  assert.match(source, /settingsCategoryButton/);
});

test("Redis scan size lives in its own settings category", () => {
  const redisTab = source.indexOf('value: "redis"');
  const redisContent = source.search(/activeSettingsTab === ['"]redis['"]/);
  const redisScanSetting = source.indexOf('t("settings.redisScanPageSize")');
  const editorContent = source.search(/activeSettingsTab === ['"]editor['"]/);

  assert.ok(redisTab > -1);
  assert.ok(redisContent > -1);
  assert.ok(redisScanSetting > redisContent);
  assert.ok(redisScanSetting > editorContent);
});

test("settings action footer stays at the bottom of the content pane", () => {
  assert.match(source, /class="[^"]*overflow-hidden[^"]*flex-col[^"]*"/);
  assert.match(source, /class="[^"]*overflow-y-auto[^"]*"/);
  assert.match(source, /<DialogFooter[\s\S]*class="[^"]*shrink-0[^"]*"/);
  assert.match(source, /<DialogFooter[\s\S]*class="[^"]*bg-transparent[^"]*"/);
  assert.doesNotMatch(source, /<DialogFooter[\s\S]*sticky/);
  assert.doesNotMatch(source, /<DialogFooter[\s\S]*bg-background/);
});

test("settings dialog has a shortcuts category", () => {
  assert.match(source, /value: "shortcuts"/);
  assert.match(source, /activeSettingsTab === ['"]shortcuts['"]/);
  assert.match(source, /SHORTCUT_DEFINITIONS/);
});

test("shortcut settings capture custom keydown input instead of fixed select options", () => {
  assert.match(source, /onShortcutKeydown/);
  assert.match(source, /@keydown="\(event: KeyboardEvent\) => onShortcutKeydown/);
  assert.doesNotMatch(source, /definition\.options/);
});

test("settings dialog exposes sidebar activation in navigation settings", () => {
  assert.match(source, /value: "navigation"/);
  assert.match(source, /activeSettingsTab === ['"]navigation['"]/);
  assert.match(source, /settings\.sidebarActivation/);
});

test("AI settings can browse provider model names while keeping manual input", () => {
  assert.match(source, /aiListModels/);
  assert.match(source, /<SearchableSelect[\s\S]*:options="aiModelOptionIds"/);
  assert.match(source, /v-model="aiEditModel"/);
  assert.match(source, /aiRefreshModels/);
});
