<template>
  <div class="mail-list">
    <div class="toolbar">
      <el-input
        v-model="keyword"
        placeholder="搜索主题 / 发件人 / 收件人..."
        clearable
        class="search"
      >
        <template #prefix>
          <el-icon><Search /></el-icon>
        </template>
      </el-input>
      <span class="meta">共 {{ total }} 封</span>
    </div>

    <el-table
      :data="messages"
      class="table"
      highlight-current-row
      @row-click="onRowClick"
      v-loading="loading"
    >
      <el-table-column label="" width="36">
        <template #default="{ row }">
          <el-icon v-if="row.has_attachment" color="#e6a23c">
            <Paperclip />
          </el-icon>
        </template>
      </el-table-column>
      <el-table-column prop="subject" label="主题" min-width="220" show-overflow-tooltip>
        <template #default="{ row }">
          <span class="subject">{{ row.subject || '(无主题)' }}</span>
        </template>
      </el-table-column>
      <el-table-column prop="from_addr" label="发件人" width="200" show-overflow-tooltip />
      <el-table-column prop="date" label="日期" width="130">
        <template #default="{ row }">
          {{ fmtDate(row.date) }}
        </template>
      </el-table-column>
      <el-table-column prop="folder" label="文件夹" width="120" show-overflow-tooltip />
    </el-table>

    <div class="pager">
      <el-pagination
        :current-page="page"
        :page-size="30"
        :total="total"
        layout="prev, pager, next, total"
        @current-change="onPageChange"
      />
    </div>
  </div>
</template>

<script setup>
import { ref, watch } from 'vue'
import dayjs from 'dayjs'

const props = defineProps({
  messages: { type: Array, default: () => [] },
  total: { type: Number, default: 0 },
  page: { type: Number, default: 1 },
  loading: { type: Boolean, default: false },
})

const emit = defineEmits(['open', 'change-page', 'search'])

const keyword = ref('')
let timer = null
watch(keyword, (v) => {
  clearTimeout(timer)
  timer = setTimeout(() => emit('search', v), 400)
})

function fmtDate(d) {
  if (!d) return ''
  // IMAP 日期格式如 "Wed, 8 Oct 2025 10:00:00 +0800"
  const t = dayjs(d)
  return t.isValid() ? t.format('YYYY-MM-DD HH:mm') : String(d).slice(0, 16)
}

function onRowClick(row) {
  emit('open', row.id)
}

function onPageChange(p) {
  emit('change-page', p)
}
</script>

<style scoped>
.mail-list {
  display: flex;
  flex-direction: column;
  height: 100%;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 16px;
  border-bottom: 1px solid var(--el-border-color);
}
.search {
  max-width: 400px;
}
.meta {
  color: var(--el-text-color-secondary);
  font-size: 13px;
}
.table {
  flex: 1;
}
.table :deep(.el-table__row) {
  cursor: pointer;
}
.subject {
  font-weight: 500;
}
.pager {
  display: flex;
  justify-content: center;
  padding: 10px 0;
  border-top: 1px solid var(--el-border-color);
}
</style>
