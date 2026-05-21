<script setup lang="ts">
import { ref, watch, shallowRef, computed } from "vue";
import type { EditorView as EditorViewType } from "@codemirror/view";
import { useI18n } from "vue-i18n";
import { CircleHelp, ExternalLink, Loader2, RefreshCw, Settings } from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SearchableSelect } from "@/components/ui/searchable-select";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  useSettingsStore,
  AI_PROVIDER_PRESETS,
  EDITOR_THEMES,
  FONT_FAMILIES,
  DEFAULT_EDITOR_SETTINGS,
  type AiProvider,
  type AiApiStyle,
} from "@/stores/settingsStore";
import { loadEditorTheme, editorFontTheme } from "@/lib/editorThemes";
import { isTauriRuntime } from "@/lib/tauriRuntime";
import { aiListModels, aiTestConnection, listSystemFonts, type AiModelInfo } from "@/lib/api";
import { eventToShortcut } from "@/lib/keyboardShortcuts";
import {
  SHORTCUT_DEFINITIONS,
  findShortcutConflict,
  formatShortcut,
  normalizeShortcutSettings,
  type ShortcutActionId,
} from "@/lib/shortcutRegistry";
import AiProviderLogo from "@/components/icons/AiProviderLogo.vue";

const { t } = useI18n();
const settingsStore = useSettingsStore();

const props = defineProps<{
  open: boolean;
  initialTab?: string;
  appVersion?: string;
}>();

const emit = defineEmits<{
  "update:open": [value: boolean];
}>();

// Local edit state
const editFontFamily = ref(settingsStore.editorSettings.fontFamily);
const editFontSize = ref(settingsStore.editorSettings.fontSize);
const editTheme = ref(settingsStore.editorSettings.theme);
const editExecuteMode = ref(settingsStore.editorSettings.executeMode);
const editWordWrap = ref(settingsStore.editorSettings.wordWrap);
const editAppLayout = ref(settingsStore.editorSettings.appLayout);
const editRedisScanPageSize = ref(settingsStore.editorSettings.redisScanPageSize);
const editShortcuts = ref(normalizeShortcutSettings(settingsStore.editorSettings.shortcuts));
const editSidebarActivation = ref(settingsStore.editorSettings.sidebarActivation);
const redisScanPageSizeOptions = [200, 1000, 5000, 10000];
const systemFonts = ref<string[]>([]);
const systemFontsLoading = ref(false);
const systemFontsLoaded = ref(false);

const presetFontLabels = new Map(FONT_FAMILIES.map((font) => [font.value, font.label]));

function cssFontFamilyForName(name: string): string {
  return `'${name.replace(/\\/g, "\\\\").replace(/'/g, "\\'")}', monospace`;
}

function readableFontFamily(value: string): string {
  const first = value.split(",")[0]?.trim() ?? value;
  return first.replace(/^['"]|['"]$/g, "").replace(/\\'/g, "'");
}

function normalizeCustomFontFamilyInput(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return "";
  if (trimmed.includes(",") || trimmed.includes("'") || trimmed.includes('"')) return trimmed;
  return cssFontFamilyForName(trimmed);
}

const systemFontOptions = computed(() => {
  const options = new Set(FONT_FAMILIES.map((font) => font.value));
  for (const font of systemFonts.value) options.add(cssFontFamilyForName(font));
  if (editFontFamily.value) options.add(editFontFamily.value);
  return [...options];
});

function displayFontFamily(value: string): string {
  return presetFontLabels.get(value) ?? readableFontFamily(value);
}

async function loadSystemFontOptions() {
  if (systemFontsLoaded.value || systemFontsLoading.value) return;
  systemFontsLoading.value = true;
  try {
    systemFonts.value = await listSystemFonts();
    systemFontsLoaded.value = true;
  } catch {
    systemFonts.value = [];
  } finally {
    systemFontsLoading.value = false;
  }
}

// Sync from store when dialog opens
watch(
  () => props.open,
  (open) => {
    if (open) {
      editFontFamily.value = settingsStore.editorSettings.fontFamily;
      editFontSize.value = settingsStore.editorSettings.fontSize;
      editTheme.value = settingsStore.editorSettings.theme;
      editExecuteMode.value = settingsStore.editorSettings.executeMode;
      editWordWrap.value = settingsStore.editorSettings.wordWrap;
      editAppLayout.value = settingsStore.editorSettings.appLayout;
      editRedisScanPageSize.value = settingsStore.editorSettings.redisScanPageSize;
      editShortcuts.value = normalizeShortcutSettings(settingsStore.editorSettings.shortcuts);
      editSidebarActivation.value = settingsStore.editorSettings.sidebarActivation;
      void loadSystemFontOptions();
    }
  },
);

const shortcutConflicts = computed(() =>
  SHORTCUT_DEFINITIONS.flatMap((definition) => {
    const conflict = findShortcutConflict(definition.id, editShortcuts.value[definition.id], editShortcuts.value);
    return conflict ? [definition.id] : [];
  }),
);
const hasShortcutConflicts = computed(() => shortcutConflicts.value.length > 0);

function hasChanges(): boolean {
  return (
    editFontFamily.value !== settingsStore.editorSettings.fontFamily ||
    editFontSize.value !== settingsStore.editorSettings.fontSize ||
    editTheme.value !== settingsStore.editorSettings.theme ||
    editExecuteMode.value !== settingsStore.editorSettings.executeMode ||
    editWordWrap.value !== settingsStore.editorSettings.wordWrap ||
    editAppLayout.value !== settingsStore.editorSettings.appLayout ||
    editRedisScanPageSize.value !== settingsStore.editorSettings.redisScanPageSize ||
    JSON.stringify(editShortcuts.value) !== JSON.stringify(settingsStore.editorSettings.shortcuts) ||
    editSidebarActivation.value !== settingsStore.editorSettings.sidebarActivation
  );
}

function applySettings() {
  if (hasShortcutConflicts.value) return;
  settingsStore.updateEditorSettings({
    fontFamily: editFontFamily.value,
    fontSize: editFontSize.value,
    theme: editTheme.value,
    executeMode: editExecuteMode.value,
    wordWrap: editWordWrap.value,
    appLayout: editAppLayout.value,
    redisScanPageSize: editRedisScanPageSize.value,
    shortcuts: editShortcuts.value,
    sidebarActivation: editSidebarActivation.value,
  });
  emit("update:open", false);
}

function resetDefaults() {
  editFontFamily.value = DEFAULT_EDITOR_SETTINGS.fontFamily;
  editFontSize.value = DEFAULT_EDITOR_SETTINGS.fontSize;
  editTheme.value = DEFAULT_EDITOR_SETTINGS.theme;
  editExecuteMode.value = DEFAULT_EDITOR_SETTINGS.executeMode;
  editWordWrap.value = DEFAULT_EDITOR_SETTINGS.wordWrap;
  editAppLayout.value = DEFAULT_EDITOR_SETTINGS.appLayout;
  editRedisScanPageSize.value = DEFAULT_EDITOR_SETTINGS.redisScanPageSize;
  editShortcuts.value = normalizeShortcutSettings(DEFAULT_EDITOR_SETTINGS.shortcuts);
  editSidebarActivation.value = DEFAULT_EDITOR_SETTINGS.sidebarActivation;
}

function onExecuteModeChange(v: any) {
  if (v === "all" || v === "current") editExecuteMode.value = v;
}

function onFontFamilyChange(v: any) {
  if (typeof v === "string") editFontFamily.value = v;
}

function onThemeChange(v: any) {
  if (typeof v === "string") editTheme.value = v as typeof DEFAULT_EDITOR_SETTINGS.theme;
}

function onRedisScanPageSizeChange(v: any) {
  const value = Number(v);
  if (redisScanPageSizeOptions.includes(value)) editRedisScanPageSize.value = value;
}

function onShortcutChange(actionId: ShortcutActionId, value: any) {
  if (typeof value !== "string") return;
  const definition = SHORTCUT_DEFINITIONS.find((item) => item.id === actionId);
  if (!definition) return;
  editShortcuts.value = { ...editShortcuts.value, [actionId]: value };
}

function onShortcutKeydown(actionId: ShortcutActionId, event: KeyboardEvent) {
  event.preventDefault();
  event.stopPropagation();
  const shortcut = eventToShortcut(event);
  if (!shortcut) return;
  onShortcutChange(actionId, shortcut);
}

function resetShortcut(actionId: ShortcutActionId) {
  const definition = SHORTCUT_DEFINITIONS.find((item) => item.id === actionId);
  if (!definition) return;
  editShortcuts.value = { ...editShortcuts.value, [actionId]: definition.defaultShortcut };
}

function setAppLayout(value: "separated" | "classic") {
  editAppLayout.value = value;
}

function setSidebarActivation(value: "single" | "double") {
  editSidebarActivation.value = value;
}

const activeSettingsTab = ref("editor");
const isWeb = !isTauriRuntime();
const displayedAppVersion = computed(() => (props.appVersion ? `v${props.appVersion}` : ""));
type SettingsCategory = "editor" | "appearance" | "navigation" | "redis" | "shortcuts" | "ai" | "security" | "about";
const settingsCategoryNav = computed<{ value: SettingsCategory; label: string }[]>(() => [
  { value: "editor", label: t("settings.editorTab") },
  { value: "appearance", label: t("settings.appearanceTab") },
  { value: "navigation", label: t("settings.navigationTab") },
  { value: "redis", label: t("settings.redisTab") },
  { value: "shortcuts", label: t("settings.shortcutsTab") },
  { value: "ai", label: t("settings.aiTab") },
  ...(isWeb ? [{ value: "security" as const, label: t("settings.securityTab") }] : []),
  { value: "about", label: t("settings.aboutTab") },
]);
const settingsTabsWithApplyFooter = new Set<SettingsCategory>([
  "editor",
  "appearance",
  "navigation",
  "redis",
  "shortcuts",
]);

function hasSettingsApplyFooter(value: SettingsCategory): boolean {
  return settingsTabsWithApplyFooter.has(value);
}

function settingsCategoryButton(value: SettingsCategory): string {
  return [
    "w-full rounded-md px-3 py-2 text-left text-sm transition-colors",
    value === activeSettingsTab.value
      ? "bg-primary text-primary-foreground shadow-sm"
      : "text-muted-foreground hover:bg-muted hover:text-foreground",
  ].join(" ");
}

function openExternalUrl(url: string) {
  if (isTauriRuntime()) {
    import("@tauri-apps/plugin-shell").then(({ open }) => open(url));
  } else {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

watch(
  () => props.open,
  async (open) => {
    if (open) {
      activeSettingsTab.value = props.initialTab || "editor";
      passwordMessage.value = "";
      oldPassword.value = "";
      newPassword.value = "";
      confirmNewPassword.value = "";
      await settingsStore.initAiConfig();
      syncAiEditState();
    }
  },
);
const oldPassword = ref("");
const newPassword = ref("");
const confirmNewPassword = ref("");
const passwordMessage = ref("");
const passwordError = ref(false);
const changingPassword = ref(false);

async function changePassword() {
  if (newPassword.value !== confirmNewPassword.value) {
    passwordMessage.value = t("auth.passwordMismatch");
    passwordError.value = true;
    return;
  }
  changingPassword.value = true;
  passwordMessage.value = "";
  try {
    const res = await fetch("/api/auth/change-password", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ old_password: oldPassword.value, new_password: newPassword.value }),
    });
    if (res.ok) {
      passwordMessage.value = t("auth.passwordChanged");
      passwordError.value = false;
      oldPassword.value = "";
      newPassword.value = "";
      confirmNewPassword.value = "";
    } else if (res.status === 401) {
      passwordMessage.value = t("auth.oldPasswordWrong");
      passwordError.value = true;
    } else {
      passwordMessage.value = t("auth.changePasswordFailed");
      passwordError.value = true;
    }
  } catch {
    passwordMessage.value = t("auth.connectFailed");
    passwordError.value = true;
  } finally {
    changingPassword.value = false;
  }
}

// ---------- AI Settings ----------
const aiProviderOptions = Object.values(AI_PROVIDER_PRESETS);
const selectedAiProviderPreset = computed(() => AI_PROVIDER_PRESETS[aiEditProvider.value]);

const aiEditProvider = ref<AiProvider>(settingsStore.aiConfig.provider);
const aiEditApiKey = ref(settingsStore.aiConfig.apiKey);
const aiEditEndpoint = ref(settingsStore.aiConfig.endpoint);
const aiEditModel = ref(settingsStore.aiConfig.model);
const aiEditApiStyle = ref<AiApiStyle>(settingsStore.aiConfig.apiStyle || "completions");
const aiEditProxyEnabled = ref(!!settingsStore.aiConfig.proxyEnabled);
const aiEditProxyUrl = ref(settingsStore.aiConfig.proxyUrl || "");
const aiEditEnableThinking = ref(settingsStore.aiConfig.enableThinking ?? true);

const aiModelOptions = ref<AiModelInfo[]>([]);
const aiModelLoading = ref(false);
const aiModelError = ref("");
const aiModelLoadedSignature = ref("");
let aiModelRequestToken = 0;

const aiCompletionsMode = computed(() => aiEditApiStyle.value === "completions");

const aiTesting = ref(false);
const aiTestResult = ref<"" | "success" | "error">("");
const aiTestError = ref("");
const aiRequiresApiKey = computed(() => AI_PROVIDER_PRESETS[aiEditProvider.value].requiresApiKey);
const aiSupportsApiStyle = computed(
  () =>
    aiEditProvider.value === "openai" ||
    aiEditProvider.value === "openai-compatible" ||
    aiEditProvider.value === "custom",
);
const aiModelListSupported = computed(() => aiEditProvider.value !== "gemini");
const aiCanListModels = computed(
  () =>
    aiModelListSupported.value &&
    !!aiEditEndpoint.value.trim() &&
    (!aiRequiresApiKey.value || !!aiEditApiKey.value.trim()),
);
const aiModelOptionIds = computed(() => aiModelOptions.value.map((model) => model.id));
const aiModelEmptyText = computed(() => {
  if (aiModelError.value) return aiModelError.value;
  if (!aiModelListSupported.value) return t("ai.modelListUnsupported");
  return t("ai.noModels");
});

function clearAiModelOptions() {
  aiModelRequestToken += 1;
  aiModelOptions.value = [];
  aiModelError.value = "";
  aiModelLoadedSignature.value = "";
  aiModelLoading.value = false;
}

function aiModelConfigSignature() {
  return JSON.stringify({
    provider: aiEditProvider.value,
    endpoint: aiEditEndpoint.value.trim(),
    apiKey: aiEditApiKey.value.trim(),
    proxyEnabled: aiEditProxyEnabled.value,
    proxyUrl: aiEditProxyUrl.value.trim(),
  });
}

function currentAiEditConfig() {
  return {
    provider: aiEditProvider.value,
    apiKey: aiEditApiKey.value,
    endpoint: aiEditEndpoint.value,
    model: aiEditModel.value,
    apiStyle: aiEditApiStyle.value,
    proxyEnabled: aiEditProxyEnabled.value,
    proxyUrl: aiEditProxyUrl.value,
    enableThinking: aiEditEnableThinking.value,
  };
}

function normalizeAiModelOptions(models: AiModelInfo[]): AiModelInfo[] {
  const seen = new Set<string>();
  const normalized: AiModelInfo[] = [];
  for (const model of models) {
    const id = model.id?.trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    normalized.push({ id, displayName: model.displayName?.trim() || undefined });
  }
  return normalized;
}

function displayAiModelName(modelId: string): string {
  return aiModelOptions.value.find((model) => model.id === modelId)?.displayName || modelId;
}

async function aiRefreshModels() {
  if (aiModelLoading.value) return;
  if (!aiModelListSupported.value) {
    aiModelError.value = t("ai.modelListUnsupported");
    return;
  }
  if (!aiEditEndpoint.value.trim()) {
    aiModelError.value = t("ai.modelListEndpointRequired");
    return;
  }
  if (aiRequiresApiKey.value && !aiEditApiKey.value.trim()) {
    aiModelError.value = t("ai.modelListApiKeyRequired");
    return;
  }

  const token = ++aiModelRequestToken;
  const signature = aiModelConfigSignature();
  aiModelLoading.value = true;
  aiModelError.value = "";
  try {
    const models = normalizeAiModelOptions(await aiListModels(currentAiEditConfig()));
    if (token !== aiModelRequestToken) return;
    aiModelOptions.value = models;
    aiModelLoadedSignature.value = signature;
    if (!aiEditModel.value.trim() && models[0]) aiEditModel.value = models[0].id;
  } catch (e: any) {
    if (token !== aiModelRequestToken) return;
    aiModelOptions.value = [];
    aiModelError.value = e?.message || String(e);
  } finally {
    if (token === aiModelRequestToken) aiModelLoading.value = false;
  }
}

function onAiModelListOpen(open: boolean) {
  if (
    open &&
    aiCanListModels.value &&
    !aiModelLoading.value &&
    (!aiModelOptions.value.length || aiModelLoadedSignature.value !== aiModelConfigSignature())
  ) {
    void aiRefreshModels();
  }
}

function aiSelectModel(modelId: string) {
  aiEditModel.value = modelId;
}

function syncAiEditState() {
  aiEditProvider.value = settingsStore.aiConfig.provider;
  aiEditApiKey.value = settingsStore.aiConfig.apiKey;
  aiEditEndpoint.value = settingsStore.aiConfig.endpoint;
  aiEditModel.value = settingsStore.aiConfig.model;
  aiEditApiStyle.value = settingsStore.aiConfig.apiStyle || "completions";
  aiEditProxyEnabled.value = !!settingsStore.aiConfig.proxyEnabled;
  aiEditProxyUrl.value = settingsStore.aiConfig.proxyUrl || "";
  aiEditEnableThinking.value = settingsStore.aiConfig.enableThinking ?? true;
  aiTestResult.value = "";
  aiTestError.value = "";
  clearAiModelOptions();
}

function aiSelectProvider(provider: AiProvider) {
  aiEditProvider.value = provider;
  aiEditEndpoint.value = AI_PROVIDER_PRESETS[provider].endpoint;
  aiEditModel.value = AI_PROVIDER_PRESETS[provider].model;
  aiEditApiStyle.value = AI_PROVIDER_PRESETS[provider].apiStyle;
  if (!AI_PROVIDER_PRESETS[provider].requiresApiKey) aiEditApiKey.value = "";
  clearAiModelOptions();
}

function aiHasChanges(): boolean {
  return (
    aiEditProvider.value !== settingsStore.aiConfig.provider ||
    aiEditApiKey.value !== settingsStore.aiConfig.apiKey ||
    aiEditEndpoint.value !== settingsStore.aiConfig.endpoint ||
    aiEditModel.value !== settingsStore.aiConfig.model ||
    aiEditApiStyle.value !== (settingsStore.aiConfig.apiStyle || "completions") ||
    aiEditProxyEnabled.value !== !!settingsStore.aiConfig.proxyEnabled ||
    aiEditProxyUrl.value !== (settingsStore.aiConfig.proxyUrl || "") ||
    aiEditEnableThinking.value !== (settingsStore.aiConfig.enableThinking ?? true)
  );
}

function aiApplySettings() {
  settingsStore.updateAiConfig(currentAiEditConfig());
}

async function aiTestConn() {
  if (
    (aiRequiresApiKey.value && !aiEditApiKey.value.trim()) ||
    !aiEditEndpoint.value.trim() ||
    !aiEditModel.value.trim()
  )
    return;
  aiTesting.value = true;
  aiTestResult.value = "";
  aiTestError.value = "";
  try {
    await aiTestConnection(currentAiEditConfig());
    aiTestResult.value = "success";
  } catch (e: any) {
    aiTestResult.value = "error";
    aiTestError.value = e?.message || String(e);
  } finally {
    aiTesting.value = false;
  }
}

// ---------- CodeMirror preview ----------
const previewRef = ref<HTMLDivElement>();
const previewView = shallowRef<EditorViewType | null>(null);

const previewSettings = computed(() => ({
  fontFamily: editFontFamily.value,
  fontSize: editFontSize.value,
  theme: editTheme.value,
}));

const previewSql = `SELECT u.id, u.name
FROM users u
ORDER BY u.id LIMIT 5;`;

let fontThemeComp: import("@codemirror/state").Compartment | null = null;
let themeComp: import("@codemirror/state").Compartment | null = null;
let editorViewModule: typeof import("@codemirror/view") | null = null;

watch(
  previewSettings,
  async (ss) => {
    if (!previewView.value || !fontThemeComp || !themeComp || !editorViewModule) return;

    const themeExt = await loadEditorTheme(ss.theme);
    previewView.value.dispatch({
      effects: [
        themeComp.reconfigure(themeExt),
        fontThemeComp.reconfigure(editorFontTheme(editorViewModule.EditorView, ss.fontSize, ss.fontFamily)),
      ],
    });
  },
  { deep: true },
);

let previewInitialized = false;

watch(activeSettingsTab, (tab) => {
  if (tab !== "editor" && previewView.value) {
    previewView.value.destroy();
    previewView.value = null;
    previewInitialized = false;
    fontThemeComp = null;
    themeComp = null;
    editorViewModule = null;
  }
});

watch(previewRef, async (el) => {
  if (!el || previewInitialized) return;
  previewInitialized = true;
  if (previewView.value) return;

  const [{ EditorView }, { EditorState, Compartment }, { sql, MySQL }, { basicSetup }] = await Promise.all([
    import("@codemirror/view"),
    import("@codemirror/state"),
    import("@codemirror/lang-sql"),
    import("codemirror"),
  ]);

  editorViewModule = { EditorView } as typeof import("@codemirror/view");
  fontThemeComp = new Compartment();
  themeComp = new Compartment();

  const ss = previewSettings.value;
  const themeExt = await loadEditorTheme(ss.theme);

  const state = EditorState.create({
    doc: previewSql,
    extensions: [
      basicSetup,
      sql({ dialect: MySQL }),
      themeComp.of(themeExt),
      fontThemeComp.of(editorFontTheme(EditorView, ss.fontSize, ss.fontFamily)),
    ],
  });

  previewView.value = new EditorView({ state, parent: previewRef.value });
});

watch(
  () => props.open,
  (open) => {
    if (!open && previewView.value) {
      previewView.value.destroy();
      previewView.value = null;
      previewInitialized = false;
      fontThemeComp = null;
      themeComp = null;
      editorViewModule = null;
    }
  },
);
</script>

<template>
  <Dialog :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogContent class="sm:max-w-[860px] max-h-[calc(100vh-80px)] overflow-hidden">
      <DialogHeader>
        <DialogTitle class="flex items-center gap-2">
          <Settings class="h-4 w-4" />
          {{ t("settings.title") }}
        </DialogTitle>
      </DialogHeader>

      <div class="flex min-h-[520px] flex-col gap-3 overflow-hidden sm:flex-row">
        <nav
          class="settingsCategoryNav flex shrink-0 gap-1 overflow-x-auto border-b pb-3 sm:w-40 sm:flex-col sm:overflow-visible sm:border-b-0 sm:border-r sm:pb-0 sm:pr-3"
        >
          <button
            v-for="category in settingsCategoryNav"
            :key="category.value"
            type="button"
            :class="settingsCategoryButton(category.value)"
            @click="activeSettingsTab = category.value"
          >
            {{ category.label }}
          </button>
        </nav>

        <div class="min-w-0 flex-1 overflow-hidden px-1 flex flex-col">
          <div class="min-h-0 flex-1 overflow-y-auto overflow-x-hidden pr-1">
            <section v-if="activeSettingsTab === 'editor'" class="flex flex-col gap-5 py-2">
              <div class="grid gap-4 md:grid-cols-[minmax(0,1fr)_220px]">
                <!-- Font Family -->
                <div class="space-y-2">
                  <Label>{{ t("settings.fontFamily") }}</Label>
                  <SearchableSelect
                    :model-value="editFontFamily"
                    :options="systemFontOptions"
                    :placeholder="t('settings.selectFont')"
                    :search-placeholder="t('settings.searchFont')"
                    :empty-text="t('settings.noFontsFound')"
                    :loading-text="t('settings.loadingFonts')"
                    :loading="systemFontsLoading"
                    allow-custom
                    :display-name="displayFontFamily"
                    :normalize-custom="normalizeCustomFontFamilyInput"
                    trigger-class="h-9 w-full max-w-none justify-between border bg-background px-3 text-sm shadow-xs hover:bg-accent"
                    content-class="w-[var(--reka-popover-trigger-width)] min-w-[260px]"
                    @update:model-value="onFontFamilyChange"
                    @update:open="(open: boolean) => open && loadSystemFontOptions()"
                  >
                    <template #trigger-label="{ label, loading }">
                      <span class="truncate" :style="{ fontFamily: editFontFamily }">
                        {{ loading ? t("settings.loadingFonts") : label }}
                      </span>
                    </template>
                    <template #option-label="{ option, label }">
                      <span class="truncate" :style="{ fontFamily: option }">{{ label }}</span>
                    </template>
                    <template #custom-option-label="{ value }">
                      <span class="truncate" :style="{ fontFamily: value }">
                        {{ t("settings.useCustomFont", { font: readableFontFamily(value) }) }}
                      </span>
                    </template>
                  </SearchableSelect>
                </div>

                <!-- Theme -->
                <div class="space-y-2">
                  <Label>{{ t("settings.theme") }}</Label>
                  <Select :model-value="editTheme" @update:model-value="onThemeChange">
                    <SelectTrigger>
                      <SelectValue :placeholder="t('settings.selectTheme')" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="theme in EDITOR_THEMES" :key="theme.value" :value="theme.value">
                        <div class="flex items-center gap-2">
                          <span
                            class="h-3 w-3 rounded-full border"
                            :class="
                              theme.dark
                                ? 'bg-foreground border-foreground/20'
                                : 'bg-muted-foreground/30 border-muted-foreground/40'
                            "
                          />
                          {{ theme.label }}
                        </div>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <!-- Font Size -->
              <div class="space-y-2">
                <div class="flex items-center justify-between">
                  <Label>{{ t("settings.fontSize") }}</Label>
                  <span class="text-xs text-muted-foreground tabular-nums">{{ editFontSize }}px</span>
                </div>
                <input
                  type="range"
                  min="10"
                  max="24"
                  step="1"
                  :value="editFontSize"
                  @input="editFontSize = Number(($event.target as HTMLInputElement).value)"
                  class="w-full accent-primary"
                />
                <div class="flex items-center gap-2 text-xs text-muted-foreground">
                  <span>10px</span>
                  <span class="flex-1 border-b border-dashed border-muted-foreground/30" />
                  <span>24px</span>
                </div>
              </div>

              <Separator />

              <div class="grid gap-4 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
                <div class="space-y-2">
                  <Label>{{ t("settings.executeMode") }}</Label>
                  <Select :model-value="editExecuteMode" @update:model-value="onExecuteModeChange">
                    <SelectTrigger>
                      <SelectValue :placeholder="t('settings.executeMode')" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="all">{{ t("settings.executeModeAll") }}</SelectItem>
                      <SelectItem value="current">{{ t("settings.executeModeCurrent") }}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div class="flex items-center justify-between gap-4 self-end rounded-md border bg-muted/20 px-3 py-2">
                  <div class="space-y-1">
                    <Label for="editor-word-wrap">{{ t("settings.wordWrap") }}</Label>
                    <p class="text-xs text-muted-foreground">{{ t("settings.wordWrapDescription") }}</p>
                  </div>
                  <Switch id="editor-word-wrap" v-model:checked="editWordWrap" class="mt-0.5" />
                </div>
              </div>

              <Separator />

              <!-- Live Preview -->
              <div class="space-y-2">
                <Label>{{ t("settings.preview") }}</Label>
                <div
                  class="rounded-md border overflow-auto max-w-full"
                  :class="
                    editTheme === 'vscode-light' || editTheme === 'duotone-light' || editTheme === 'xcode'
                      ? 'border-border'
                      : 'border-border/50'
                  "
                >
                  <div ref="previewRef" style="min-width: 100%" />
                </div>
              </div>
            </section>

            <section v-else-if="activeSettingsTab === 'appearance'" class="flex flex-col gap-5 py-2">
              <div class="space-y-2">
                <Label>{{ t("settings.appLayout") }}</Label>
                <div class="grid grid-cols-2 gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    class="h-auto justify-start border p-3"
                    :class="editAppLayout === 'separated' ? 'border-blue-300 ring-2 ring-blue-300/50' : ''"
                    @click="setAppLayout('separated')"
                  >
                    <div class="text-left">
                      <div class="text-sm font-medium">{{ t("settings.appLayoutSeparated") }}</div>
                      <div class="text-xs text-muted-foreground">{{ t("settings.appLayoutSeparatedDescription") }}</div>
                    </div>
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    class="h-auto justify-start border p-3"
                    :class="editAppLayout === 'classic' ? 'border-blue-300 ring-2 ring-blue-300/50' : ''"
                    @click="setAppLayout('classic')"
                  >
                    <div class="text-left">
                      <div class="text-sm font-medium">{{ t("settings.appLayoutClassic") }}</div>
                      <div class="text-xs text-muted-foreground">{{ t("settings.appLayoutClassicDescription") }}</div>
                    </div>
                  </Button>
                </div>
              </div>
            </section>

            <section v-else-if="activeSettingsTab === 'navigation'" class="flex flex-col gap-5 py-2">
              <div class="space-y-2">
                <Label>{{ t("settings.sidebarActivation") }}</Label>
                <div class="grid grid-cols-2 gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    class="h-auto justify-start border p-3"
                    :class="editSidebarActivation === 'single' ? 'border-blue-300 ring-2 ring-blue-300/50' : ''"
                    @click="setSidebarActivation('single')"
                  >
                    <div class="text-left">
                      <div class="text-sm font-medium">{{ t("settings.sidebarActivationSingle") }}</div>
                      <div class="text-xs text-muted-foreground">
                        {{ t("settings.sidebarActivationSingleDescription") }}
                      </div>
                    </div>
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    class="h-auto justify-start border p-3"
                    :class="editSidebarActivation === 'double' ? 'border-blue-300 ring-2 ring-blue-300/50' : ''"
                    @click="setSidebarActivation('double')"
                  >
                    <div class="text-left">
                      <div class="text-sm font-medium">{{ t("settings.sidebarActivationDouble") }}</div>
                      <div class="text-xs text-muted-foreground">
                        {{ t("settings.sidebarActivationDoubleDescription") }}
                      </div>
                    </div>
                  </Button>
                </div>
              </div>
            </section>

            <section v-else-if="activeSettingsTab === 'redis'" class="flex flex-col gap-5 py-2">
              <div class="space-y-2">
                <Label>{{ t("settings.redisScanPageSize") }}</Label>
                <Select :model-value="String(editRedisScanPageSize)" @update:model-value="onRedisScanPageSizeChange">
                  <SelectTrigger>
                    <SelectValue :placeholder="t('settings.redisScanPageSize')" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="size in redisScanPageSizeOptions" :key="size" :value="String(size)">
                      {{ t("settings.redisScanPageSizeOption", { count: size }) }}
                    </SelectItem>
                  </SelectContent>
                </Select>
                <p class="text-xs text-muted-foreground">{{ t("settings.redisScanPageSizeDescription") }}</p>
              </div>
            </section>

            <section v-else-if="activeSettingsTab === 'shortcuts'" class="flex flex-col gap-5 py-2">
              <div class="space-y-3">
                <div
                  v-for="definition in SHORTCUT_DEFINITIONS"
                  :key="definition.id"
                  class="grid gap-2 rounded-md border bg-background p-3 sm:grid-cols-[minmax(0,1fr)_220px] sm:items-center"
                >
                  <div class="min-w-0 space-y-1">
                    <Label>{{ t(definition.labelKey) }}</Label>
                    <p class="text-xs text-muted-foreground">
                      {{ t(`settings.shortcutScope${definition.scope[0].toUpperCase()}${definition.scope.slice(1)}`) }}
                    </p>
                  </div>
                  <div class="space-y-1.5">
                    <div class="flex gap-2">
                      <Input
                        :model-value="formatShortcut(editShortcuts[definition.id])"
                        readonly
                        :aria-invalid="shortcutConflicts.includes(definition.id)"
                        :placeholder="t('settings.shortcutPressShortcut')"
                        class="font-mono"
                        @keydown="(event: KeyboardEvent) => onShortcutKeydown(definition.id, event)"
                      />
                      <Button type="button" variant="outline" class="shrink-0" @click="resetShortcut(definition.id)">
                        {{ t("settings.reset") }}
                      </Button>
                    </div>
                    <p v-if="shortcutConflicts.includes(definition.id)" class="text-xs text-destructive">
                      {{ t("settings.shortcutConflict") }}
                    </p>
                  </div>
                </div>
              </div>
            </section>
            <!-- AI Settings Tab -->
            <section v-else-if="activeSettingsTab === 'ai'" class="flex flex-col gap-5 py-2">
              <p class="text-xs text-muted-foreground">{{ t("ai.settingsHint") }}</p>

              <div class="space-y-3">
                <div class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">{{ t("ai.provider") }}</Label>
                  <Select :model-value="aiEditProvider" @update:model-value="(v: any) => aiSelectProvider(v)">
                    <SelectTrigger class="col-span-2 h-8 text-xs">
                      <SelectValue>
                        <span class="flex items-center gap-2">
                          <AiProviderLogo
                            :provider="selectedAiProviderPreset.provider"
                            :label="selectedAiProviderPreset.label"
                            :icon-slug="selectedAiProviderPreset.iconSlug"
                          />
                          <span>{{ selectedAiProviderPreset.label }}</span>
                        </span>
                      </SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem
                        v-for="provider in aiProviderOptions"
                        :key="provider.provider"
                        :value="provider.provider"
                      >
                        <span class="flex items-center gap-2">
                          <AiProviderLogo
                            :provider="provider.provider"
                            :label="provider.label"
                            :icon-slug="provider.iconSlug"
                          />
                          <span>{{ provider.label }}</span>
                        </span>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">API Key</Label>
                  <Input
                    v-model="aiEditApiKey"
                    type="password"
                    autocomplete="off"
                    class="col-span-2 h-8 text-xs"
                    :placeholder="aiRequiresApiKey ? '' : 'Optional'"
                  />
                </div>

                <div class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">Endpoint</Label>
                  <Input
                    v-model="aiEditEndpoint"
                    placeholder="https://api.openai.com/v1"
                    autocomplete="off"
                    class="col-span-2 h-8 text-xs"
                  />
                </div>

                <div class="grid grid-cols-3 items-start gap-3">
                  <Label class="pt-2 text-right text-xs">{{ t("ai.model") }}</Label>
                  <div class="col-span-2 space-y-1.5">
                    <div class="flex min-w-0 items-center gap-2">
                      <Input v-model="aiEditModel" autocomplete="off" class="h-8 min-w-0 flex-1 text-xs" />
                      <SearchableSelect
                        :model-value="aiEditModel"
                        :options="aiModelOptionIds"
                        :placeholder="t('ai.browseModels')"
                        :search-placeholder="t('ai.searchModels')"
                        :empty-text="aiModelEmptyText"
                        :loading-text="t('ai.loadingModels')"
                        :loading="aiModelLoading"
                        :display-name="displayAiModelName"
                        trigger-class="h-8 min-w-[104px] max-w-[150px] shrink-0 border border-border bg-background px-2 text-xs shadow-none hover:bg-muted/50"
                        content-class="w-72"
                        @update:model-value="aiSelectModel"
                        @update:open="onAiModelListOpen"
                      >
                        <template #trigger-label="{ loading }">
                          <span class="truncate">{{ loading ? t("ai.loadingModels") : t("ai.browseModels") }}</span>
                        </template>
                        <template #option-label="{ option, label }">
                          <span class="flex min-w-0 flex-col">
                            <span class="truncate">{{ label }}</span>
                            <span v-if="label !== option" class="truncate text-[11px] text-muted-foreground">{{
                              option
                            }}</span>
                          </span>
                        </template>
                      </SearchableSelect>
                      <Button
                        type="button"
                        size="icon"
                        variant="outline"
                        class="shrink-0"
                        :disabled="aiModelLoading || !aiModelListSupported"
                        :title="t('ai.refreshModels')"
                        :aria-label="t('ai.refreshModels')"
                        @click="aiRefreshModels"
                      >
                        <Loader2 v-if="aiModelLoading" class="h-3.5 w-3.5 animate-spin" />
                        <RefreshCw v-else class="h-3.5 w-3.5" />
                      </Button>
                    </div>
                    <p v-if="aiModelError" class="text-xs text-destructive">{{ aiModelError }}</p>
                    <p v-else-if="!aiModelOptionIds.length" class="text-xs text-muted-foreground">
                      {{ aiModelListSupported ? t("ai.modelListHint") : t("ai.modelListUnsupported") }}
                    </p>
                  </div>
                </div>

                <div v-if="aiSupportsApiStyle" class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">API</Label>
                  <div class="col-span-2 flex gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      class="h-8 flex-1 text-xs"
                      :class="{ 'border-blue-300 border-2 ring-2 ring-blue-300/50': aiEditApiStyle === 'completions' }"
                      @click="aiEditApiStyle = 'completions'"
                      >/chat/completions</Button
                    >
                    <Button
                      size="sm"
                      variant="outline"
                      class="h-8 flex-1 text-xs"
                      :class="{ 'border-blue-300 border-2 ring-2 ring-blue-300/50': aiEditApiStyle === 'responses' }"
                      @click="aiEditApiStyle = 'responses'"
                      >/responses</Button
                    >
                  </div>
                </div>

                <div class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">{{ t("ai.enableThinking") }}</Label>
                  <div class="col-span-2 flex items-center gap-2">
                    <label class="flex items-center gap-2 text-xs text-muted-foreground">
                      <input
                        v-model="aiEditEnableThinking"
                        type="checkbox"
                        class="h-4 w-4 shrink-0 accent-primary"
                        :disabled="!aiCompletionsMode || aiEditProvider === 'gemini'"
                      />
                      {{ aiEditEnableThinking ? t("ai.enableThinkingOn") : t("ai.enableThinkingOff") }}
                    </label>
                    <Popover>
                      <PopoverTrigger as-child>
                        <CircleHelp class="h-3.5 w-3.5 cursor-help text-muted-foreground hover:text-foreground" />
                      </PopoverTrigger>
                      <PopoverContent class="max-w-[320px] text-xs leading-relaxed" side="top" align="start">
                        {{ t("ai.enableThinkingHint") }}
                      </PopoverContent>
                    </Popover>
                  </div>
                </div>

                <div class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">{{ t("ai.proxy") }}</Label>
                  <label class="col-span-2 flex items-center gap-2 text-xs text-muted-foreground">
                    <input v-model="aiEditProxyEnabled" type="checkbox" class="h-4 w-4 shrink-0 accent-primary" />
                    {{ t("ai.proxyEnable") }}
                  </label>
                </div>

                <div class="grid grid-cols-3 items-center gap-3">
                  <Label class="text-right text-xs">{{ t("ai.proxyUrl") }}</Label>
                  <Input
                    v-model="aiEditProxyUrl"
                    autocomplete="off"
                    class="col-span-2 h-8 text-xs"
                    placeholder="socks5://127.0.0.1:7890"
                    :disabled="!aiEditProxyEnabled"
                  />
                </div>
              </div>
            </section>

            <section v-else-if="activeSettingsTab === 'security' && isWeb" class="flex flex-col gap-5 py-2">
              <div class="space-y-3">
                <Label class="text-base">{{ t("auth.changePassword") }}</Label>
                <p class="text-sm text-muted-foreground">{{ t("auth.changePasswordDescription") }}</p>
                <Input
                  v-model="oldPassword"
                  type="password"
                  :placeholder="t('auth.oldPassword')"
                  class="h-9"
                  autocomplete="off"
                />
                <Input
                  v-model="newPassword"
                  type="password"
                  :placeholder="t('auth.newPassword')"
                  class="h-9"
                  autocomplete="off"
                />
                <Input
                  v-model="confirmNewPassword"
                  type="password"
                  :placeholder="t('auth.confirmPassword')"
                  class="h-9"
                  autocomplete="off"
                />
                <p
                  v-if="passwordMessage"
                  class="text-xs"
                  :class="passwordError ? 'text-destructive' : 'text-green-500'"
                >
                  {{ passwordMessage }}
                </p>
              </div>
            </section>

            <section v-else-if="activeSettingsTab === 'about'" class="flex flex-col gap-5 py-2">
              <div class="rounded-lg border bg-muted/20 p-4">
                <div class="flex items-start justify-between gap-4">
                  <div class="min-w-0 space-y-1">
                    <div class="text-lg font-semibold">DBX</div>
                    <p class="text-sm text-muted-foreground">{{ t("settings.aboutDescription") }}</p>
                  </div>
                  <div
                    v-if="displayedAppVersion"
                    class="rounded-md border bg-background px-2 py-1 text-xs text-muted-foreground"
                  >
                    {{ displayedAppVersion }}
                  </div>
                </div>
              </div>

              <div class="grid gap-3 sm:grid-cols-2">
                <button
                  type="button"
                  class="rounded-lg border p-4 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  @click="openExternalUrl('https://qm.qq.com/cgi-bin/qm/qr?k=&group_code=1087880322')"
                >
                  <div class="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    {{ t("settings.community") }}
                  </div>
                  <div class="mt-3 flex items-center gap-2 text-sm font-medium">
                    <img
                      src="data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIGhlaWdodD0iODYiIHdpZHRoPSI4NiIgdmlld0JveD0iMCAwIDEyMCAxNDUiPjxwYXRoIGZpbGw9IiNmYWFiMDciIGQ9Ik02MC41MDMgMTQyLjIzN2MtMTIuNTMzIDAtMjQuMDM4LTQuMTk1LTMxLjQ0NS0xMC40Ni0zLjc2MiAxLjEyNC04LjU3NCAyLjkzMi0xMS42MSA1LjE3NS0yLjYgMS45MTgtMi4yNzUgMy44NzQtMS44MDcgNC42NjMgMi4wNTYgMy40NyAzNS4yNzMgMi4yMTYgNDQuODYyIDEuMTM2em0wIDBjMTIuNTM1IDAgMjQuMDM5LTQuMTk1IDMxLjQ0Ny0xMC40NiAzLjc2IDEuMTI0IDguNTczIDIuOTMyIDExLjYxIDUuMTc1IDIuNTk4IDEuOTE4IDIuMjc0IDMuODc0IDEuODA1IDQuNjYzLTIuMDU2IDMuNDctMzUuMjcyIDIuMjE2LTQ0Ljg2MiAxLjEzNnptMCAwIi8+PHBhdGggZD0iTTYwLjU3NiA2Ny4xMTljMjAuNjk4LS4xNCAzNy4yODYtNC4xNDcgNDIuOTA3LTUuNjgzIDEuMzQtLjM2NyAyLjA1Ni0xLjAyNCAyLjA1Ni0xLjAyNC4wMDUtLjE4OS4wODUtMy4zNy4wODUtNS4wMUMxMDUuNjI0IDI3Ljc2OCA5Mi41OC4wMDEgNjAuNSAwIDI4LjQyLjAwMSAxNS4zNzUgMjcuNzY5IDE1LjM3NSA1NS40MDFjMCAxLjY0Mi4wOCA0LjgyMi4wODYgNS4wMSAwIDAgLjU4My42MTUgMS42NS45MTMgNS4xOSAxLjQ0NCAyMi4wOSA1LjY1IDQzLjMxMiA1Ljc5NXptNTYuMjQ1IDIzLjAyYy0xLjI4My00LjEyOS0zLjAzNC04Ljk0NC00LjgwOC0xMy41NjggMCAwLTEuMDItLjEyNi0xLjUzNy4wMjMtMTUuOTEzIDQuNjIzLTM1LjIwMiA3LjU3LTQ5LjkgNy4zOTJoLS4xNTNjLTE0LjYxNi4xNzUtMzMuNzc0LTIuNzM3LTQ5LjYzNC03LjMxNS0uNjA2LS4xNzUtMS44MDItLjEtMS44MDItLjEtMS43NzQgNC42MjQtMy41MjUgOS40NC00LjgwOCAxMy41NjgtNi4xMTkgMTkuNjktNC4xMzYgMjcuODM4LTIuNjI3IDI4LjAyIDMuMjM5LjM5MiAxMi42MDYtMTQuODIxIDEyLjYwNi0xNC44MjEgMCAxNS40NTkgMTMuOTU3IDM5LjE5NSA0NS45MTggMzkuNDEzaC44NDhjMzEuOTYtLjIxOCA0NS45MTctMjMuOTU0IDQ1LjkxNy0zOS40MTMgMCAwIDkuMzY4IDE1LjIxMyAxMi42MDcgMTQuODIyIDEuNTA4LS4xODMgMy40OTEtOC4zMzItMi42MjctMjguMDIxIi8+PHBhdGggZmlsbD0iI2ZmZiIgZD0iTTQ5LjA4NSA0MC44MjRjLTQuMzUyLjE5Ny04LjA3LTQuNzYtOC4zMDQtMTEuMDYzLS4yMzYtNi4zMDUgMy4wOTgtMTEuNTc2IDcuNDUtMTEuNzczIDQuMzQ3LS4xOTUgOC4wNjQgNC43NiA4LjMgMTEuMDY1LjIzOCA2LjMwNi0zLjA5NyAxMS41NzctNy40NDYgMTEuNzcxbTMxLjEzMy0xMS4wNjNjLS4yMzMgNi4zMDItMy45NTEgMTEuMjYtOC4zMDMgMTEuMDYzLTQuMzUtLjE5NS03LjY4NC01LjQ2NS03LjQ0Ni0xMS43Ny4yMzYtNi4zMDUgMy45NTItMTEuMjYgOC4zLTExLjA2NiA0LjM1Mi4xOTcgNy42ODYgNS40NjggNy40NDkgMTEuNzczIi8+PHBhdGggZmlsbD0iI2ZhYWIwNyIgZD0iTTg3Ljk1MiA0OS43MjVDODYuNzkgNDcuMTUgNzUuMDc3IDQ0LjI4IDYwLjU3OCA0NC4yOGgtLjE1NmMtMTQuNSAwLTI2LjIxMiAyLjg3LTI3LjM3NSA1LjQ0NmEuODYzLjg2MyAwIDAwLS4wODUuMzY3Ljg4Ljg4IDAgMDAuMTYuNDk2Yy45OCAxLjQyNyAxMy45ODUgOC40ODcgMjcuMyA4LjQ4N2guMTU2YzEzLjMxNCAwIDI2LjMxOS03LjA1OCAyNy4yOTktOC40ODdhLjg3My44NzMgMCAwMC4xNi0uNDk4Ljg1Ni44NTYgMCAwMC0uMDg1LS4zNjUiLz48cGF0aCBkPSJNNTQuNDM0IDI5Ljg1NGMuMTk5IDIuNDktMS4xNjcgNC43MDItMy4wNDYgNC45NDMtMS44ODMuMjQyLTMuNTY4LTEuNTgtMy43NjgtNC4wNy0uMTk3LTIuNDkyIDEuMTY3LTQuNzA0IDMuMDQzLTQuOTQ0IDEuODg2LS4yNDQgMy41NzQgMS41OCAzLjc3MSA0LjA3bTExLjk1Ni44MzNjLjM4NS0uNjg5IDMuMDA0LTQuMzEyIDguNDI3LTIuOTkzIDEuNDI1LjM0NyAyLjA4NC44NTcgMi4yMjMgMS4wNTcuMjA1LjI5Ni4yNjIuNzE4LjA1MyAxLjI4Ni0uNDEyIDEuMTI2LTEuMjYzIDEuMDk1LTEuNzM0Ljg3NS0uMzA1LS4xNDItNC4wODItMi42Ni03LjU2MiAxLjA5Ny0uMjQuMjU3LS42NjguMzQ2LTEuMDczLjA0LS40MDctLjMwOC0uNTc0LS45My0uMzM0LTEuMzYyIi8+PHBhdGggZmlsbD0iI2ZmZiIgZD0iTTYwLjU3NiA4My4wOGgtLjE1M2MtOS45OTYuMTItMjIuMTE2LTEuMjA0LTMzLjg1NC0zLjUxOC0xLjAwNCA1LjgxOC0xLjYxIDEzLjEzMi0xLjA5IDIxLjg1MyAxLjMxNiAyMi4wNDMgMTQuNDA3IDM1LjkgMzQuNjE0IDM2LjFoLjgyYzIwLjIwOC0uMiAzMy4yOTgtMTQuMDU3IDM0LjYxNi0zNi4xLjUyLTguNzIzLS4wODctMTYuMDM1LTEuMDkyLTIxLjg1NC0xMS43MzkgMi4zMTUtMjMuODYyIDMuNjQtMzMuODYgMy41MTgiLz48cGF0aCBmaWxsPSIjZWIxOTIzIiBkPSJNMzIuMTAyIDgxLjIzNXYyMS42OTNzOS45MzcgMi4wMDQgMTkuODkzLjYxNlY4My41MzVjLTYuMzA3LS4zNTctMTMuMTA5LTEuMTUyLTE5Ljg5My0yLjMiLz48cGF0aCBmaWxsPSIjZWIxOTIzIiBkPSJNMTA1LjUzOSA2MC40MTJzLTE5LjMzIDYuMTAyLTQ0Ljk2MyA2LjI3NWgtLjE1M2MtMjUuNTkxLS4xNzItNDQuODk2LTYuMjU1LTQ0Ljk2Mi02LjI3NUw4Ljk4NyA3Ni41N2MxNi4xOTMgNC44ODIgMzYuMjYxIDguMDI4IDUxLjQzNiA3Ljg0NWguMTUzYzE1LjE3NS4xODMgMzUuMjQyLTIuOTYzIDUxLjQzNy03Ljg0NXptMCAwIi8+PC9zdmc+"
                      alt="QQ"
                      class="h-7 w-7 rounded-md bg-white p-1"
                    />
                    {{ t("settings.qqGroup") }}
                    <ExternalLink class="ml-auto h-3.5 w-3.5 text-muted-foreground" />
                  </div>
                  <div class="mt-1 font-mono text-base">1087880322</div>
                </button>
                <button
                  type="button"
                  class="rounded-lg border p-4 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  @click="openExternalUrl('https://discord.gg/W7NyVDRt6a')"
                >
                  <div class="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    {{ t("settings.community") }}
                  </div>
                  <div class="mt-3 flex items-center gap-2 text-sm font-medium">
                    <img
                      src="https://cdn.simpleicons.org/discord/5865F2"
                      alt="Discord"
                      class="h-7 w-7 rounded-md bg-white p-1"
                    />
                    Discord
                    <ExternalLink class="ml-auto h-3.5 w-3.5 text-muted-foreground" />
                  </div>
                  <div class="mt-1 text-sm text-primary">discord.gg/W7NyVDRt6a</div>
                </button>
                <button
                  type="button"
                  class="rounded-lg border p-4 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  @click="openExternalUrl('https://github.com/t8y2/dbx')"
                >
                  <div class="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    {{ t("settings.project") }}
                  </div>
                  <div class="mt-3 flex items-center gap-2 text-sm font-medium">
                    <img
                      src="https://cdn.simpleicons.org/github/181717"
                      alt="GitHub"
                      class="h-7 w-7 rounded-md bg-white p-1"
                    />
                    {{ t("settings.openSource") }}
                    <ExternalLink class="ml-auto h-3.5 w-3.5 text-muted-foreground" />
                  </div>
                  <div class="mt-1 text-sm text-primary">github.com/t8y2/dbx</div>
                </button>
                <button
                  type="button"
                  class="rounded-lg border p-4 text-left transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  @click="openExternalUrl('https://dbxio.com')"
                >
                  <div class="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    {{ t("settings.project") }}
                  </div>
                  <div class="mt-3 flex items-center gap-2 text-sm font-medium">
                    <img src="/logo.png" alt="DBX" class="h-7 w-7 rounded-md" />
                    {{ t("settings.officialDocs") }}
                    <ExternalLink class="ml-auto h-3.5 w-3.5 text-muted-foreground" />
                  </div>
                  <div class="mt-1 text-sm text-primary">dbxio.com</div>
                </button>
              </div>
            </section>
          </div>

          <DialogFooter
            v-if="hasSettingsApplyFooter(activeSettingsTab as SettingsCategory)"
            class="mx-0 mb-0 shrink-0 rounded-none border-t-0 bg-transparent px-0 pb-0 pt-3 gap-3 sm:gap-3"
          >
            <Button variant="outline" @click="resetDefaults">
              {{ t("settings.resetDefaults") }}
            </Button>
            <div class="flex-1" />
            <Button variant="outline" @click="emit('update:open', false)">
              {{ t("common.close") }}
            </Button>
            <Button :disabled="!hasChanges() || hasShortcutConflicts" @click="applySettings">
              {{ t("settings.apply") }}
            </Button>
          </DialogFooter>

          <DialogFooter
            v-else-if="activeSettingsTab === 'ai'"
            class="mx-0 mb-0 shrink-0 rounded-none border-t-0 bg-transparent px-0 pb-0 pt-3 gap-3 sm:gap-3"
          >
            <div class="flex flex-1 items-center gap-2">
              <Button
                size="sm"
                variant="outline"
                :disabled="
                  aiTesting ||
                  (aiRequiresApiKey && !aiEditApiKey?.trim()) ||
                  !aiEditEndpoint?.trim() ||
                  !aiEditModel?.trim()
                "
                @click="aiTestConn"
              >
                <Loader2 v-if="aiTesting" class="h-3 w-3 animate-spin mr-1" />
                {{ t("connection.test") }}
              </Button>
              <span v-if="aiTestResult === 'success'" class="text-xs text-green-500">
                {{ t("connection.testSuccess") }}
              </span>
              <span
                v-else-if="aiTestResult === 'error'"
                class="text-xs text-destructive truncate max-w-[200px]"
                :title="aiTestError"
              >
                {{ aiTestError }}
              </span>
            </div>
            <Button variant="outline" @click="emit('update:open', false)">{{ t("common.close") }}</Button>
            <Button :disabled="!aiHasChanges()" @click="aiApplySettings">{{ t("settings.apply") }}</Button>
          </DialogFooter>

          <DialogFooter
            v-else-if="activeSettingsTab === 'security' && isWeb"
            class="mx-0 mb-0 shrink-0 rounded-none border-t-0 bg-transparent px-0 pb-0 pt-3"
          >
            <Button variant="outline" @click="emit('update:open', false)">
              {{ t("common.close") }}
            </Button>
            <Button
              :disabled="changingPassword || !oldPassword || !newPassword || !confirmNewPassword"
              @click="changePassword"
            >
              {{ t("auth.changePassword") }}
            </Button>
          </DialogFooter>

          <DialogFooter
            v-else-if="activeSettingsTab === 'about'"
            class="mx-0 mb-0 shrink-0 rounded-none border-t-0 bg-transparent px-0 pb-0 pt-3"
          >
            <Button variant="outline" @click="emit('update:open', false)">
              {{ t("common.close") }}
            </Button>
          </DialogFooter>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
