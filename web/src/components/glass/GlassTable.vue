<template>
  <div class="glass-table-wrapper">
    <table class="glass-table">
      <thead>
        <tr>
          <th v-for="col in columns" :key="col.key" class="glass-table__th">
            {{ col.label }}
          </th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="(row, i) in data" :key="i" class="glass-table__row">
          <td v-for="col in columns" :key="col.key" class="glass-table__td">
            <slot :name="col.key" :row="row" :value="row[col.key]">
              {{ row[col.key] }}
            </slot>
          </td>
        </tr>
        <tr v-if="!data.length">
          <td :colspan="columns.length" class="glass-table__empty">
            {{ emptyText }}
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<script setup>
defineProps({
  columns: { type: Array, default: () => [] },
  data: { type: Array, default: () => [] },
  emptyText: { type: String, default: 'No data' },
})
</script>

<style scoped>
.glass-table-wrapper {
  overflow-x: auto;
  border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle);
}
.glass-table { width: 100%; border-collapse: collapse; }
.glass-table__th {
  text-align: left;
  padding: 0.65rem 0.85rem;
  font-size: 0.75rem; font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
  background: var(--bg-raised);
  border-bottom: 1px solid var(--border-subtle);
}
.glass-table__row {
  transition: background var(--duration-fast);
}
.glass-table__row:hover { background: var(--lobster-primary-soft); }
.glass-table__td {
  padding: 0.6rem 0.85rem;
  font-size: 0.85rem;
  color: var(--text-primary);
  border-bottom: 1px solid var(--border-subtle);
}
.glass-table__empty {
  padding: 2rem;
  text-align: center;
  color: var(--text-muted);
  font-size: 0.85rem;
}
</style>
