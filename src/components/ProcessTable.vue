<script setup lang="ts">
/**
 * Virtual-scrolled process table.
 *
 * Replaces `v-data-table` for the process list (audit item O6). The old table
 * rendered every row: with ~390 processes the DOM carried ~390 rows x 6 cells
 * of live bindings, and each metrics tick patched all of them even though only
 * ~20 rows are on screen. This component keeps the same columns, sorting and
 * row interactions, but renders only the rows inside the viewport (plus a small
 * overscan), so per-tick patch cost is bounded by the viewport height instead of
 * by the process count.
 *
 * The windowing arithmetic is exact rather than measured: rows are a fixed
 * 36 px and the header a fixed 40 px, matching Vuetify's `density="compact"`
 * table metrics (`--v-table-row-height` / `--v-table-header-height`).
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { ProcessInfo } from "../types";
import {
  ioPriorityLabel,
  memoryPriorityLabel,
  priorityClassColor,
  priorityClassLabel,
} from "../types";
import type { ViewMode } from "../constants";
import { useI18n } from "../i18n";

/** A rendered row: a plain `ProcessInfo` in flat mode, a tree row (carrying
 *  depth / child state) in tree mode. */
type ProcessRow = ProcessInfo & { _depth?: number; _hasChildren?: boolean };

const props = defineProps<{
  items: ProcessRow[];
  /** Total height of the table, header included (px). */
  height: number;
  viewMode: ViewMode;
  /** Direct-child count per pid, for the tree-mode chip. Built once per list
   *  change by the parent so rendering does not scan the list per row. */
  childCounts?: Map<number, number>;
  /** Expanded tree nodes; drives the chevron direction only (the parent owns
   *  the state and toggles it from the row-click event). */
  expandedPids?: number[];
}>();

const emit = defineEmits<{
  rowContextmenu: [event: MouseEvent, item: ProcessRow];
  rowClick: [item: ProcessRow];
}>();

const { t } = useI18n();

const ROW_HEIGHT = 36;
const HEADER_HEIGHT = 40;
/** Rows kept above and below the viewport so fast scrolling never shows gaps. */
const OVERSCAN = 6;

interface Column {
  key: string;
  title: string;
  /** flex-grow weight; the ratios mirror the old v-data-table widths. */
  grow: number;
  minWidth: number;
  align: "start" | "end";
  sortable: boolean;
  /** Direction applied the first time this column is clicked. */
  firstOrder: "asc" | "desc";
}

const columns = computed<Column[]>(() => [
  { key: "pid", title: "PID", grow: 70, minWidth: 70, align: "start", sortable: true, firstOrder: "asc" },
  {
    key: "name",
    title: props.viewMode === "tree" ? t("nameTree") : t("name"),
    grow: 200,
    minWidth: 150,
    align: "start",
    sortable: true,
    firstOrder: "asc",
  },
  { key: "cpu_usage_percent", title: t("cpu"), grow: 80, minWidth: 80, align: "end", sortable: true, firstOrder: "desc" },
  { key: "memory_bytes", title: t("memory"), grow: 100, minWidth: 100, align: "end", sortable: true, firstOrder: "desc" },
  { key: "priority_class", title: t("priority"), grow: 110, minWidth: 110, align: "start", sortable: true, firstOrder: "desc" },
  { key: "affinity", title: t("affinity"), grow: 150, minWidth: 140, align: "start", sortable: false, firstOrder: "asc" },
]);

function colStyle(col: Column) {
  return { flex: `${col.grow} 1 0`, minWidth: `${col.minWidth}px` };
}

// ---------- Sorting (self-implemented; v-data-table's built-in sort is gone
// ---------- with the table, and a single sort column is all the app used) ----------
const sortKey = ref<string>("cpu_usage_percent");
const sortOrder = ref<"asc" | "desc">("desc");

function isSorted(col: Column): boolean {
  return sortKey.value === col.key;
}

/** Current sort direction's arrow. Unsorted columns render the same icon, but
 *  CSS keeps it hidden until hover. */
function sortIcon(): string {
  return sortOrder.value === "asc" ? "mdi-arrow-up" : "mdi-arrow-down";
}

function ariaSort(col: Column): "none" | "ascending" | "descending" | undefined {
  if (!col.sortable) return undefined;
  if (!isSorted(col)) return "none";
  return sortOrder.value === "asc" ? "ascending" : "descending";
}

function onHeaderClick(col: Column) {
  if (!col.sortable) return;
  if (isSorted(col)) {
    sortOrder.value = sortOrder.value === "asc" ? "desc" : "asc";
  } else {
    sortKey.value = col.key;
    sortOrder.value = col.firstOrder;
  }
}

function compareBy(key: string, a: ProcessRow, b: ProcessRow): number {
  switch (key) {
    case "pid":
      return a.pid - b.pid;
    case "name":
      return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
    case "cpu_usage_percent":
      return a.cpu_usage_percent - b.cpu_usage_percent;
    case "memory_bytes":
      return a.memory_bytes - b.memory_bytes;
    case "priority_class":
      return (a.priority_class ?? 0) - (b.priority_class ?? 0);
    default:
      return 0;
  }
}

const sortedRows = computed<ProcessRow[]>(() => {
  const rows = props.items.slice();
  const key = sortKey.value;
  const dir = sortOrder.value === "asc" ? 1 : -1;
  rows.sort((a, b) => {
    // Unreadable priorities ("-") always sort last, in both directions.
    if (key === "priority_class" && (a.priority_class === null) !== (b.priority_class === null)) {
      return a.priority_class === null ? 1 : -1;
    }
    const r = compareBy(key, a, b);
    // Tie-break on pid so the order is stable and independent of direction.
    return r !== 0 ? dir * r : a.pid - b.pid;
  });
  return rows;
});

// ---------- Windowing ----------
const viewportRef = ref<HTMLElement | null>(null);
const scrollTop = ref(0);
/** Width stolen by the vertical scrollbar, so the header can be inset to stay
 *  aligned with the body columns. */
const scrollbarWidth = ref(0);

const bodyHeight = computed(() => Math.max(0, props.height - HEADER_HEIGHT));
const totalHeight = computed(() => sortedRows.value.length * ROW_HEIGHT);
const startIndex = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW_HEIGHT) - OVERSCAN));
const endIndex = computed(() =>
  Math.min(sortedRows.value.length, startIndex.value + Math.ceil(bodyHeight.value / ROW_HEIGHT) + OVERSCAN * 2),
);
const offsetY = computed(() => startIndex.value * ROW_HEIGHT);
const visibleRows = computed(() => sortedRows.value.slice(startIndex.value, endIndex.value));

function onScroll() {
  const el = viewportRef.value;
  if (!el) return;
  scrollTop.value = el.scrollTop;
}

function measureScrollbar() {
  const el = viewportRef.value;
  if (!el) return;
  scrollbarWidth.value = Math.max(0, el.offsetWidth - el.clientWidth);
}

/** The list can shrink under the scroll position (search, filter, process
 *  exits); without this the window would render past the end of the data. */
function clampScroll() {
  const el = viewportRef.value;
  if (!el) return;
  const max = Math.max(0, totalHeight.value - bodyHeight.value);
  if (el.scrollTop > max) el.scrollTop = max;
  scrollTop.value = el.scrollTop;
}

let resizeObserver: ResizeObserver | null = null;

onMounted(() => {
  measureScrollbar();
  if (typeof ResizeObserver !== "undefined" && viewportRef.value) {
    resizeObserver = new ResizeObserver(() => {
      measureScrollbar();
      clampScroll();
    });
    resizeObserver.observe(viewportRef.value);
  }
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  resizeObserver = null;
});

// Row count changes are what make the scrollbar appear/disappear and what can
// leave the offset past the end of the data.
watch(() => sortedRows.value.length, () => {
  measureScrollbar();
  clampScroll();
});

// ---------- Cell helpers ----------
/** Non-Normal priority tier coloring for the priority column */
function prioStyle(p: ProcessInfo) {
  const color = priorityClassColor(p.priority_class);
  return color ? { color, fontWeight: 600 } : undefined;
}

/** Priority column tooltip: all three priority classes in detail */
function priorityTooltip(p: ProcessInfo): string {
  return `${t("prioCpu")}: ${priorityClassLabel(p.priority_class)}\n${t("prioIo")}: ${ioPriorityLabel(p.io_priority)}\n${t("prioMem")}: ${memoryPriorityLabel(p.memory_priority)}`;
}

const expandedSet = computed(() => new Set(props.expandedPids ?? []));

function childCount(pid: number): number {
  return props.childCounts?.get(pid) ?? 0;
}
</script>

<template>
  <div class="pt" role="table">
    <!-- Header: a sibling of the scroll container, so it stays put without
         position: sticky (which would also need a scroll container ancestor). -->
    <div class="pt-head" role="rowgroup" :style="{ height: `${HEADER_HEIGHT}px`, paddingRight: `${scrollbarWidth}px` }">
      <div class="pt-head-row" role="row">
        <div v-for="col in columns" :key="col.key" role="columnheader" class="pt-cell pt-th"
          :class="[`pt-align-${col.align}`, { 'pt-th--sortable': col.sortable, 'pt-th--sorted': isSorted(col) }]"
          :style="colStyle(col)" :aria-sort="ariaSort(col)" :tabindex="col.sortable ? 0 : undefined"
          @click="onHeaderClick(col)" @keydown.enter.prevent="onHeaderClick(col)"
          @keydown.space.prevent="onHeaderClick(col)">
          <span class="pt-ellipsis">{{ col.title }}</span>
          <v-icon v-if="col.sortable" size="x-small" class="pt-sort-icon" :icon="sortIcon()" />
        </div>
      </div>
    </div>

    <div ref="viewportRef" class="pt-body" role="rowgroup" :style="{ height: `${bodyHeight}px` }"
      @scroll.passive="onScroll">
      <div v-if="sortedRows.length" class="pt-canvas" :style="{ height: `${totalHeight}px` }">
        <div class="pt-window" :style="{ transform: `translateY(${offsetY}px)` }">
          <div v-for="row in visibleRows" :key="row.pid" class="pt-row" role="row"
            :style="{ height: `${ROW_HEIGHT}px` }" @contextmenu="emit('rowContextmenu', $event, row)"
            @click="emit('rowClick', row)">
            <!-- PID -->
            <div class="pt-cell pt-align-start" role="cell" :style="colStyle(columns[0])">
              <code class="text-body-2">{{ row.pid }}</code>
            </div>

            <!-- Name -->
            <div class="pt-cell pt-align-start" role="cell" :style="colStyle(columns[1])">
              <div class="d-flex align-center pt-name"
                :style="viewMode === 'tree' ? `padding-left: ${(row._depth ?? 0) * 20}px` : ''">
                <template v-if="viewMode === 'tree'">
                  <v-icon v-if="row._hasChildren" size="small" class="mr-1"
                    :icon="expandedSet.has(row.pid) ? 'mdi-chevron-down' : 'mdi-chevron-right'" />
                  <v-icon v-else size="small" class="mr-1" icon="mdi-minus" color="grey-lighten-1" />
                </template>
                <v-icon size="small" class="mr-2" :icon="row.name.endsWith('.exe') ? 'mdi-application' : 'mdi-cog'"
                  color="grey" />
                <v-tooltip :text="row.exe_path ?? ''" location="top" :disabled="!row.exe_path">
                  <template #activator="{ props: tipProps }">
                    <span class="text-body-2 pt-ellipsis" v-bind="tipProps">{{ row.name }}</span>
                  </template>
                </v-tooltip>
                <v-chip v-if="viewMode === 'tree' && row._hasChildren" size="x-small" variant="tonal" class="ml-2">
                  {{ childCount(row.pid) }}
                </v-chip>
              </div>
            </div>

            <!-- CPU -->
            <div class="pt-cell pt-align-end" role="cell" :style="colStyle(columns[2])">
              <span class="text-body-2 font-weight-medium" :style="{ color: row._display.cpu_color }">
                {{ row._display.cpu_text }}
              </span>
            </div>

            <!-- Memory -->
            <div class="pt-cell pt-align-end" role="cell" :style="colStyle(columns[3])">
              <span v-if="row.memory_bytes > 0" class="text-body-2">{{ row._display.mem_text }}</span>
              <span v-else class="text-medium-emphasis">-</span>
            </div>

            <!-- Priority -->
            <div class="pt-cell pt-align-start" role="cell" :style="colStyle(columns[4])">
              <span class="text-body-2" :style="prioStyle(row)" :title="priorityTooltip(row)">
                {{ priorityClassLabel(row.priority_class) }}
              </span>
            </div>

            <!-- Affinity -->
            <div class="pt-cell pt-align-start" role="cell" :style="colStyle(columns[5])">
              <div v-if="row.access_denied" class="text-medium-emphasis text-body-2">
                <v-icon icon="mdi-lock" size="small" class="mr-1" />{{ t('accessDenied') }}
              </div>
              <div v-else-if="!row.affinity_mask" class="text-medium-emphasis text-body-2">-</div>
              <div v-else class="d-flex align-center">
                <div class="d-flex ga-1 flex-wrap">
                  <div v-for="bar in row._display.ccd_bars" :key="bar.id" class="affinity-bar"
                    :class="{ 'affinity-bar--empty': bar.enabled === 0 }" role="img"
                    :aria-label="t('ccdBarTitle', { id: bar.id, enabled: bar.enabled, total: bar.total })"
                    :title="t('ccdBarTitle', { id: bar.id, enabled: bar.enabled, total: bar.total })"
                    :style="{ background: bar.enabled > 0 ? bar.color : 'transparent', borderColor: bar.enabled > 0 ? bar.color : undefined }">
                    {{ bar.enabled }}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Empty state -->
      <div v-else class="pt-empty">
        <v-icon icon="mdi-database-off-outline" size="large" class="mb-2" />
        <div>{{ t('noData') }}</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.pt {
  display: flex;
  flex-direction: column;
  width: 100%;
}

/* ---------- Header ---------- */
/* Mirrors VTable's `.v-table--fixed-header > ... > th` so the header looks
   exactly like the one it replaces. */
.pt-head {
  flex: 0 0 auto;
  background: rgb(var(--v-theme-surface));
  box-shadow: inset 0 -1px 0 rgba(var(--v-border-color), var(--v-border-opacity));
}
.pt-head-row {
  display: flex;
  align-items: center;
  height: 100%;
}
.pt-th {
  font-size: 0.875rem;
  font-weight: 700;
  user-select: none;
}
.pt-th--sortable {
  cursor: pointer;
}
.pt-th--sortable:hover {
  color: rgba(var(--v-theme-on-surface), var(--v-high-emphasis-opacity));
}
.pt-sort-icon {
  margin-inline-start: 4px;
  opacity: 0;
  transition: opacity 0.15s ease;
}
.pt-th--sortable:hover .pt-sort-icon,
.pt-th--sorted .pt-sort-icon {
  opacity: 0.6;
}
.pt-th--sorted .pt-sort-icon {
  opacity: 1;
}

/* ---------- Body ---------- */
.pt-body {
  position: relative;
  flex: 1 1 auto;
  overflow-y: auto;
  overflow-x: hidden;
}
.pt-canvas {
  position: relative;
  width: 100%;
}
.pt-window {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
}
.pt-row {
  display: flex;
  align-items: center;
  /* Row height is fixed by inline style; clip anything that would overflow so a
     wrapped affinity swatch can never bleed into the next row. */
  overflow: hidden;
}
/* Same hover treatment as VTable's `hover` rows, so the swap is not visible. */
.pt-row:hover {
  background: rgba(var(--v-border-color), var(--v-hover-opacity));
}

/* ---------- Cells ---------- */
.pt-cell {
  display: flex;
  align-items: center;
  min-width: 0;
  padding: 0 16px;
  overflow: hidden;
  white-space: nowrap;
}
.pt-align-start {
  justify-content: flex-start;
  text-align: start;
}
.pt-align-end {
  justify-content: flex-end;
  text-align: end;
}
.pt-ellipsis {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pt-name {
  min-width: 0;
  overflow: hidden;
}

/* ---------- Empty state ---------- */
.pt-empty {
  position: absolute;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  color: rgba(var(--v-theme-on-surface), var(--v-medium-emphasis-opacity));
}

/* ---------- Affinity swatches ---------- */
.affinity-bar {
  width: 20px;
  height: 20px;
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid;
  border-radius: 4px;
  font-size: 11px;
  font-weight: 600;
  color: #fff;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5);
  cursor: help;
  transition: background 0.15s ease, border-color 0.15s ease, color 0.15s ease;
}
.affinity-bar--empty {
  border-color: rgba(128, 128, 128, 0.28);
  color: rgba(128, 128, 128, 0.75);
  text-shadow: none;
}
</style>
