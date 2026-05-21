<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { HTMLAttributes } from "vue";
import { Check, ChevronDown, Search } from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { filterDatabaseOptions } from "@/lib/databaseOptionSearch";
import { cn } from "@/lib/utils";

const props = withDefaults(
  defineProps<{
    modelValue: string;
    options: string[];
    placeholder: string;
    searchPlaceholder: string;
    emptyText: string;
    loadingText: string;
    loading?: boolean;
    allowCustom?: boolean;
    triggerClass?: HTMLAttributes["class"];
    contentClass?: HTMLAttributes["class"];
    displayName?: (option: string) => string;
    normalizeCustom?: (value: string) => string;
  }>(),
  {
    loading: false,
    allowCustom: false,
    displayName: (option: string) => option,
    normalizeCustom: (value: string) => value,
  },
);

const emit = defineEmits<{
  "update:modelValue": [value: string];
  "update:open": [value: boolean];
}>();

const open = ref(false);
const searchText = ref("");
const searchInput = ref<InstanceType<typeof Input>>();

const selectedLabel = computed(() => {
  if (!props.modelValue) return props.placeholder;
  return props.displayName(props.modelValue);
});

const filteredOptions = computed(() => filterDatabaseOptions(props.options, searchText.value, props.displayName));
const customOptionValue = computed(() => props.normalizeCustom(searchText.value.trim()));
const canSelectCustom = computed(
  () => props.allowCustom && !!customOptionValue.value && !props.options.includes(customOptionValue.value),
);

watch(open, async (value) => {
  emit("update:open", value);
  if (!value) {
    searchText.value = "";
    return;
  }
  await nextTick();
  const input = searchInput.value?.$el as HTMLInputElement | undefined;
  input?.focus();
});

function selectOption(option: string) {
  emit("update:modelValue", option);
  open.value = false;
}

function selectCustomOption() {
  if (!canSelectCustom.value) return;
  selectOption(customOptionValue.value);
}
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger as-child>
      <Button
        type="button"
        variant="ghost"
        :class="
          cn(
            'h-6 w-auto max-w-56 justify-between gap-1 border-0 bg-transparent px-1 text-xs font-normal shadow-none hover:bg-muted/50 focus-visible:ring-0',
            triggerClass,
          )
        "
      >
        <slot name="trigger-label" :value="modelValue" :label="selectedLabel" :loading="loading">
          <span class="truncate">{{ loading ? loadingText : selectedLabel }}</span>
        </slot>
        <ChevronDown class="h-3 w-3 shrink-0 opacity-60" />
      </Button>
    </PopoverTrigger>
    <PopoverContent align="end" :class="cn('w-52 gap-1 p-1.5', contentClass)">
      <div class="flex items-center gap-1.5 rounded-sm border bg-background px-2">
        <Search class="h-3 w-3 shrink-0 text-muted-foreground" />
        <Input
          ref="searchInput"
          :model-value="searchText"
          :placeholder="searchPlaceholder"
          class="h-6 border-0 px-0 text-sm shadow-none focus-visible:ring-0"
          @update:model-value="(value) => (searchText = String(value))"
        />
      </div>
      <div class="max-h-64 overflow-y-auto py-1">
        <div v-if="loading" class="px-2 py-2 text-sm text-muted-foreground">
          {{ loadingText }}
        </div>
        <template v-else-if="filteredOptions.length">
          <button
            v-for="option in filteredOptions"
            :key="option"
            type="button"
            class="flex h-8 w-full min-w-0 items-center gap-2 rounded-sm px-2 text-left text-sm hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground focus-visible:outline-none"
            @click="selectOption(option)"
          >
            <Check :class="cn('h-3.5 w-3.5 shrink-0', option === modelValue ? 'opacity-100' : 'opacity-0')" />
            <slot name="option-label" :option="option" :label="displayName(option)">
              <span class="truncate">{{ displayName(option) }}</span>
            </slot>
          </button>
          <button
            v-if="canSelectCustom"
            type="button"
            class="flex h-8 w-full min-w-0 items-center gap-2 rounded-sm px-2 text-left text-sm hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground focus-visible:outline-none"
            @click="selectCustomOption"
          >
            <Check class="h-3.5 w-3.5 shrink-0 opacity-0" />
            <slot name="custom-option-label" :value="customOptionValue">
              <span class="truncate">{{ customOptionValue }}</span>
            </slot>
          </button>
        </template>
        <button
          v-else-if="canSelectCustom"
          type="button"
          class="flex h-8 w-full min-w-0 items-center gap-2 rounded-sm px-2 text-left text-sm hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground focus-visible:outline-none"
          @click="selectCustomOption"
        >
          <Check class="h-3.5 w-3.5 shrink-0 opacity-0" />
          <slot name="custom-option-label" :value="customOptionValue">
            <span class="truncate">{{ customOptionValue }}</span>
          </slot>
        </button>
        <div v-else class="px-2 py-2 text-sm text-muted-foreground">
          {{ emptyText }}
        </div>
      </div>
    </PopoverContent>
  </Popover>
</template>
