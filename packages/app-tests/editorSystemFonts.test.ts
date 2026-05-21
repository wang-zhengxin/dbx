import { readFileSync } from "node:fs";
import { strict as assert } from "node:assert";
import test from "node:test";

test("frontend API exposes system font loading", () => {
  const api = readFileSync("apps/desktop/src/lib/api.ts", "utf8");
  const tauri = readFileSync("apps/desktop/src/lib/tauri.ts", "utf8");
  const http = readFileSync("apps/desktop/src/lib/http.ts", "utf8");

  assert.match(api, /export const listSystemFonts = forward\("listSystemFonts"\)/);
  assert.match(tauri, /invoke\("list_system_fonts"\)/);
  assert.match(http, /export async function listSystemFonts\(\): Promise<string\[\]>/);
});

test("settings editor font picker loads system fonts and accepts custom names", () => {
  const source = readFileSync("apps/desktop/src/components/editor/EditorSettingsDialog.vue", "utf8");

  assert.match(source, /listSystemFonts/);
  assert.match(source, /systemFontOptions/);
  assert.match(source, /allow-custom/);
  assert.match(source, /normalizeCustomFontFamilyInput/);
  assert.match(source, /settings\.useCustomFont/);
  assert.doesNotMatch(source, /settings\.customFontFamily/);
});

test("Tauri registers the system font command", () => {
  const commands = readFileSync("src-tauri/src/commands/mod.rs", "utf8");
  const lib = readFileSync("src-tauri/src/lib.rs", "utf8");

  assert.match(commands, /pub mod system_fonts/);
  assert.match(lib, /commands::system_fonts::list_system_fonts/);
});
